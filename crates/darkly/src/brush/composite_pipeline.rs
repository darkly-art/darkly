//! Brush commit composite pipeline — the scratch → layer blit with
//! shader-side Porter-Duff source-over against a canvas-copy
//! background.
//!
//! Lives outside `nodes/` because it is module-generic: every brush
//! terminal's `commit` hook (paint_compiled, watercolor_compiled,
//! smudge, liquify) routes through [`BrushPaintTargetExt::commit_brush_dab`],
//! which calls this pipeline. Owning the type in its previously-named
//! `color_output` node was the "module-generic infrastructure named
//! after the one consumer that happened to use it first" anti-pattern
//! AGENTS.md warns about — fixed now by lifting it.
//!
//! Owns two render pipelines — one targeting `Rgba8Unorm` (raster
//! layer destinations) and one targeting `R8Unorm` (mask
//! destinations). Same WGSL; the GPU writes only `.r` to R8 targets.
//! Per the type-owned-dispatch principle, the format branch lives in
//! [`CompositePipeline::pipeline`], not at every call site.

use std::any::Any;

use crate::brush::pipeline::{BrushPipelineEntry, BuildContext, DynamicUniformRing};

/// Uniform data for the brush commit composite shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CompositeUniforms {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub target_offset: [f32; 2],
    pub target_size: [f32; 2],
    pub canvas_size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub blend_mode: u32,
    pub fg_premultiplied: u32,
    pub stroke_opacity: f32,
    pub apply_selection: u32,
}

pub struct CompositePipeline {
    pipeline_rgba: wgpu::RenderPipeline,
    pipeline_r8: wgpu::RenderPipeline,
    ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
}

impl CompositePipeline {
    pub fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-composite"),
                source: wgpu::ShaderSource::Wgsl(
                    concat!(
                        include_str!("../../../../shaders/source_over.wgsl"),
                        "\n",
                        include_str!("../../../../shaders/brush/composite.wgsl"),
                    )
                    .into(),
                ),
            });
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-composite-layout"),
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    ctx.dab_bgl,
                    ctx.selection_bgl,
                    ctx.canvas_copy_bgl,
                ],
                immediate_size: 0,
            });
        let make = |format: wgpu::TextureFormat, label: &'static str| {
            ctx.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
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
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    multiview_mask: None,
                    cache: None,
                })
        };
        let pipeline_rgba = make(wgpu::TextureFormat::Rgba8Unorm, "brush-composite-rgba");
        let pipeline_r8 = make(wgpu::TextureFormat::R8Unorm, "brush-composite-r8");
        let (ring, uniform_bind_group) = ctx.make_uniform_ring::<CompositeUniforms>(
            "brush-composite-uniforms",
            "brush-composite-uniform-bg",
        );
        Self {
            pipeline_rgba,
            pipeline_r8,
            ring,
            uniform_bind_group,
        }
    }

    /// Look up the composite pipeline for a destination format. Stroke
    /// scratch composites hit the RGBA variant; stroke→layer commits hit
    /// the variant matching the layer's storage format.
    pub fn pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.pipeline_r8,
            _ => &self.pipeline_rgba,
        }
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
