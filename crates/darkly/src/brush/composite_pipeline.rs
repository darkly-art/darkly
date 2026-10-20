//! Brush commit composite pipeline: the scratch → layer blit with
//! shader-side Porter-Duff source-over against a canvas-copy
//! background.
//!
//! Lives outside `nodes/` because it is module-generic: every brush
//! terminal's `commit` hook (paint, watercolor, smudge, liquify)
//! routes through [`BrushPaintTargetExt::commit_brush_dab`], which
//! calls this pipeline.
//!
//! Owns four render pipelines: two destinations (`Rgba8Unorm` for raster
//! layers, `R8Unorm` for masks; same WGSL, the GPU writes only `.r` to
//! R8 targets) by two foreground formats (float, read by `fs_main`
//! through the float canvas-copy layout; the packed `r32uint` ground a
//! compute terminal accumulates, read by `fs_packed` through the uint
//! layout). Per the type-owned-dispatch principle, both format branches
//! live in [`CompositePipeline::pipeline`], not at every call site.

use std::any::Any;

use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};

/// `BrushPipelines::new` harvests this alongside `nodes::registrations()`
/// so the central registry has a single uniform input. The composite
/// pipeline isn't tied to any one node (every terminal's `commit` hook
/// uses it), so it lives here instead of inside `nodes/`.
pub fn composite_pipeline_registration() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "composite",
        build: |ctx| Box::new(CompositePipeline::build(ctx)),
    }
}

/// Uniform data for the brush commit composite shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CompositeUniforms {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub target_offset: [f32; 2],
    pub target_size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub blend_mode: u32,
    /// Stroke opacity of the foreground committed through the deposit
    /// ceiling. `0.0` means that slot is absent and is not read.
    pub wash_opacity: f32,
    /// Stroke opacity of the foreground composited source-over on top.
    /// `0.0` means that slot is absent and is not read.
    pub build_opacity: f32,
    /// Which law the two slots commit under; see [`CommitLaw`].
    pub law: u32,
}

/// The law the commit (and the appearance snapshot) lays a terminal's two
/// slots under. The numbering is the shader's (`lib/commit_law.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitLaw {
    /// The first slot deposits through the ceiling, the second composites
    /// source-over on top: the accumulation dial's two halves.
    Deposit = 0,
    /// The first slot's alpha is the coverage a finger displaced, the
    /// second the pigment it brought, laid over `1 - coverage` of the
    /// background.
    Move = 1,
}

pub struct CompositePipeline {
    /// `[float, packed]` foregrounds onto an `Rgba8Unorm` destination.
    pipelines_rgba: [wgpu::RenderPipeline; 2],
    /// `[float, packed]` foregrounds onto an `R8Unorm` destination.
    pipelines_r8: [wgpu::RenderPipeline; 2],
    ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
}

impl CompositePipeline {
    pub fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-composite"),
                // No canvas lib: the commit samples no selection, so
                // `plane_to_selection_uv` (the only symbol it supplies)
                // has no caller here.
                source: wgpu::ShaderSource::Wgsl(
                    concat!(
                        include_str!("../../shaders/source_over.wgsl"),
                        "\n",
                        include_str!("../../shaders/lib/deposit_ceiling.wgsl"),
                        "\n",
                        include_str!("../../shaders/lib/commit_law.wgsl"),
                        "\n",
                        include_str!("../../shaders/brush/composite.wgsl"),
                    )
                    .into(),
                ),
            });
        // group(1) and group(2) are the two foreground slots, group(3)
        // the pre-stroke background. The background is always a
        // `texture_2d<f32> + sampler` over the float canvas-copy layout;
        // the foregrounds are both float or both packed, over the layout
        // their scratch was built against, so a scratch's write side and
        // any of its channels can each fill either slot.
        let foreground_layouts = [
            (ctx.canvas_copy, "fs_main", "float"),
            (
                ctx.canvas_copy_layouts
                    .for_format(crate::brush::node::PACKED_GROUND_FORMAT),
                "fs_packed",
                "packed",
            ),
        ];
        let make = |dest: wgpu::TextureFormat| -> [wgpu::RenderPipeline; 2] {
            foreground_layouts.map(|(fg, entry, fg_label)| {
                let layout = ctx
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(&format!("brush-composite-{fg_label}-layout")),
                        bind_group_layouts: &[
                            Some(ctx.uniform_bgl),
                            Some(&fg.bgl),
                            Some(&fg.bgl),
                            Some(&ctx.canvas_copy.bgl),
                        ],
                        immediate_size: 0,
                    });
                ctx.device
                    .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(&format!("brush-composite-{fg_label}-{dest:?}")),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: &shader,
                            entry_point: Some("vs_main"),
                            buffers: &[],
                            compilation_options: Default::default(),
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some(entry),
                            targets: &[Some(wgpu::ColorTargetState {
                                format: dest,
                                blend: Some(wgpu::BlendState::REPLACE),
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                            compilation_options: Default::default(),
                        }),
                        multiview_mask: None,
                        cache: None,
                    })
            })
        };
        let pipelines_rgba = make(wgpu::TextureFormat::Rgba8Unorm);
        let pipelines_r8 = make(wgpu::TextureFormat::R8Unorm);
        let (ring, uniform_bind_group) = ctx.make_uniform_ring::<CompositeUniforms>(
            "brush-composite-uniforms",
            "brush-composite-uniform-bg",
        );
        Self {
            pipelines_rgba,
            pipelines_r8,
            ring,
            uniform_bind_group,
        }
    }

    /// Look up the composite pipeline for a destination format and the
    /// foregrounds' format. Stroke→layer commits hit the destination
    /// variant matching the layer's storage format; a uint foreground is
    /// the packed ground, anything else is sampled as float.
    pub fn pipeline(
        &self,
        dest: wgpu::TextureFormat,
        foreground: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let by_dest = match dest {
            wgpu::TextureFormat::R8Unorm => &self.pipelines_r8,
            _ => &self.pipelines_rgba,
        };
        let packed = foreground.sample_type(None, None) == Some(wgpu::TextureSampleType::Uint);
        &by_dest[usize::from(packed)]
    }

    pub fn uniform_bind_group(&self) -> &wgpu::BindGroup {
        &self.uniform_bind_group
    }

    pub fn write_uniforms(&self, queue: &wgpu::Queue, uniforms: &CompositeUniforms) -> u32 {
        self.ring.write(queue, bytemuck::bytes_of(uniforms))
    }
}

impl BrushPipelineEntry for CompositePipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.ring)
    }
}
