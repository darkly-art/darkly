//! Watercolor GPU terminal node — three-step physical model per dab:
//!
//! 1. Sample the canvas under the dab footprint (alpha-weighted average
//!    → the colour AND alpha of whatever is painted there).
//! 2. Build the brush load by mixing the sampled canvas with the user's
//!    paint colour by `deposit`:
//!    `load_rgb   = mix(canvas_rgb, paint_rgb, deposit)`,
//!    `load_alpha = mix(canvas_alpha, paint_alpha, deposit)`.
//!    `deposit = 0` → load is pure canvas (smudge). `deposit = 1` → load
//!    is pure paint (regular stamp). Mid → tinted smear. Tracking alpha
//!    alongside RGB makes `deposit=0` over an empty canvas a true no-op
//!    — there's nothing to deposit.
//! 3. Stamp the load through the brush tip, modulated by `wetness`:
//!    `fg_a = mask × wetness × load_alpha × selection × stroke_opacity`,
//!    then source-over blend against the canvas. `wetness=0` → `fg_a=0`,
//!    no effect at all. `wetness=1` → full stamp. Mid → translucent.
//!
//! Sibling of [`color_output`] and [`liquify`]. Lifecycle follows liquify:
//!
//! - `begin_stroke` — copies `pre_stroke_texture` → `stroke_scratch` so
//!   `scratch read mirror` reads real pixels from dab 1. Every rewind reseeds.
//! - `evaluate_gpu` (per dab) — two GPU passes:
//!     1. Pickup pass — alpha-weighted 8×8 average of canvas_copy across
//!        the brush footprint, written to a 1×1 RGBA8 pickup texture.
//!     2. Composite pass — applies the load math and stamps into the
//!        stroke scratch. Reads canvas_copy for the source-over background.
//! - `commit` — `commit_scratch_blit(scratch → layer)`, same as liquify.
//!   `gpu.blend_mode` ignored (erase isn't meaningful for a smudge brush).
//! - `render_preview` — blits the upstream `brush_preview` texture into
//!   the overlay's preview mask.
//!
//! Each dab is independent; there is no cross-dab carry. `wetness` is a
//! per-dab smudge intensity, not a smudge-length / persistence parameter.
//!
//! The dab from upstream `stamp` bakes its paint colour into RGB, but
//! this node ignores `dab.rgb` and uses `dab.a` as the alpha mask plus
//! `color` (read separately) as the paint colour — avoids
//! de-premultiplying (undefined where `dab.a == 0`).

use std::any::Any;

use crate::brush::eval::{BrushNodeEvaluator, BrushPreviewInfo, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::{
    BlitUniforms, BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Pipelines ──────────────────────────────────────────────────────────

/// Uniform data for the watercolor pickup shader.
///
/// Drives one render pass per watercolor dab that averages canvas_copy
/// under the brush footprint (alpha-weighted RGB, unweighted alpha) into
/// a 1×1 RGBA8 pickup texture.  Each dab is independent — no cross-dab
/// carry; every dab samples the canvas afresh.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WatercolorPickupUniforms {
    pub center: [f32; 2],              // brush centre in canvas pixels
    pub copy_origin: [f32; 2], // top-left of the valid scratch-mirror region (canvas pixels)
    pub scratch_mirror_size: [f32; 2], // scratch mirror texture dimensions
    pub half_extent: [f32; 2], // half the dab footprint (canvas pixels) per axis
}

/// Uniform data for the watercolor compositing shader.
///
/// Same shape as `CompositeUniforms` minus the per-dab `blend_mode` and
/// `fg_premultiplied` knobs (watercolor is always source-over with a
/// premultiplied dab), plus `paint_color` and `deposit` — the two new
/// quantities the watercolor blend reads on top of the standard composite
/// inputs.
///
/// `paint_color` is first because vec4 needs 16-byte alignment in WGSL.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WatercolorCompositeUniforms {
    pub paint_color: [f32; 4], // straight-alpha paint color (rgb used; alpha via dab.a)
    pub origin: [f32; 2],      // quad top-left in canvas pixels
    pub size: [f32; 2],        // quad size in canvas pixels
    pub target_offset: [f32; 2], // canvas-space offset of render target's (0,0) pixel
    pub target_size: [f32; 2], // render target pixel dimensions (vertex NDC)
    pub canvas_size: [f32; 2], // document canvas dimensions (fragment selection UV)
    pub uv_min: [f32; 2],      // min UV in dab texture (nonzero when clipped at top/left)
    pub uv_max: [f32; 2],      // max UV in dab texture
    pub deposit: f32,          // paint↔pickup mix ratio (0 = pure pickup, 1 = pure paint)
    pub wetness: f32,          // smudge intensity (0 = dry brush, 1 = full smudge)
    pub stroke_opacity: f32,   // per-stroke opacity cap (1.0 = no cap)
    pub apply_selection: u32,  // 1 = modulate fg by selection, 0 = ignore (commit pass)
}

/// Watercolor pickup pipeline.  Alpha-weighted average of canvas_copy
/// under the brush footprint, written to a 1×1 RGBA8 pickup texture.
/// The composite pass samples this single texel so every fragment of the
/// dab reads the same colour.  Each dab is independent.
///
/// Also owns the 1×1 pickup texture itself (allocated once, reused per
/// dab — the pickup pass overwrites the single texel) and both its
/// views: the sampled-side view embedded by every `Scratch` in its
/// `watercolor_sources_bind_group`, and the render-attachment view the
/// pickup pass writes to.
pub struct WatercolorPickupPipeline {
    pipeline: wgpu::RenderPipeline,
    ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
    sampled_view: wgpu::TextureView,
    attachment_view: wgpu::TextureView,
}

impl WatercolorPickupPipeline {
    fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-watercolor-pickup"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../../shaders/brush/watercolor_pickup.wgsl").into(),
                ),
            });
        // group(0) = uniforms, group(1) = canvas copy.
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-watercolor-pickup-layout"),
                bind_group_layouts: &[ctx.uniform_bgl, ctx.canvas_copy_bgl],
                immediate_size: 0,
            });
        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-watercolor-pickup"),
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
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            });
        let (ring, uniform_bind_group) = ctx.make_uniform_ring::<WatercolorPickupUniforms>(
            "brush-watercolor-pickup-uniforms",
            "brush-watercolor-pickup-uniform-bg",
        );
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brush-watercolor-pickup"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let sampled_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let attachment_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            pipeline,
            ring,
            uniform_bind_group,
            _texture: texture,
            sampled_view,
            attachment_view,
        }
    }

    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub fn uniform_bind_group(&self) -> &wgpu::BindGroup {
        &self.uniform_bind_group
    }

    pub fn write_uniforms(&self, queue: &wgpu::Queue, uniforms: &WatercolorPickupUniforms) -> u32 {
        self.ring.write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Render-attachment view of the pickup texture.  The pickup pass
    /// writes one fragment here per dab.
    pub fn attachment_view(&self) -> &wgpu::TextureView {
        &self.attachment_view
    }

    /// Sampled-side view of the pickup texture.  Embedded by every
    /// `Scratch` in its `watercolor_sources_bind_group` at binding 2.
    /// Forwarded by [`BrushPipelines::watercolor_pickup_view`].
    pub fn sampled_view(&self) -> &wgpu::TextureView {
        &self.sampled_view
    }
}

impl BrushPipelineEntry for WatercolorPickupPipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.ring)
    }
}

fn watercolor_pickup_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "watercolor_pickup",
        build: |ctx| Box::new(WatercolorPickupPipeline::build(ctx)),
    }
}

/// Watercolor composite pipeline.  REPLACE blend (shader-side
/// Porter-Duff, identical pattern to the standard composite).  Always
/// targets RGBA8 stroke scratch — stroke→layer commits go through the
/// shared composite pipeline, so no R8 variant is needed here.
pub struct WatercolorCompositePipeline {
    pipeline: wgpu::RenderPipeline,
    ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
}

impl WatercolorCompositePipeline {
    fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-watercolor-composite"),
                source: wgpu::ShaderSource::Wgsl(
                    concat!(
                        include_str!("../../../../../shaders/source_over.wgsl"),
                        "\n",
                        include_str!("../../../../../shaders/brush/watercolor_composite.wgsl"),
                    )
                    .into(),
                ),
            });
        // group(0) = uniforms, group(1) = dab, group(2) = selection,
        // group(3) = sources (canvas_copy + pickup).
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-watercolor-composite-layout"),
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    ctx.dab_bgl,
                    ctx.selection_bgl,
                    ctx.watercolor_sources_bgl,
                ],
                immediate_size: 0,
            });
        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-watercolor-composite"),
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
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            });
        let (ring, uniform_bind_group) = ctx.make_uniform_ring::<WatercolorCompositeUniforms>(
            "brush-watercolor-composite-uniforms",
            "brush-watercolor-composite-uniform-bg",
        );
        Self {
            pipeline,
            ring,
            uniform_bind_group,
        }
    }

    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub fn uniform_bind_group(&self) -> &wgpu::BindGroup {
        &self.uniform_bind_group
    }

    pub fn write_uniforms(
        &self,
        queue: &wgpu::Queue,
        uniforms: &WatercolorCompositeUniforms,
    ) -> u32 {
        self.ring.write(queue, bytemuck::bytes_of(uniforms))
    }
}

impl BrushPipelineEntry for WatercolorCompositePipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.ring)
    }
}

fn watercolor_composite_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "watercolor_composite",
        build: |ctx| Box::new(WatercolorCompositePipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![
            watercolor_pickup_pipeline_reg(),
            watercolor_composite_pipeline_reg(),
        ],
        node: NodeRegistration {
        type_id: "watercolor",
        category: "output",
        display_name: "Watercolor",
        ports: vec![
            PortDef::input("dab", BrushWireType::Texture)
                .with_description("Brush tip shape"),
            PortDef::input("dab_size", BrushWireType::Vec2)
                .with_description("Brush tip size in pixels"),
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Where to place the brush tip on the canvas"),
            PortDef::input("color", BrushWireType::Color)
                .with_description("Paint color"),
            PortDef::input("deposit", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Deposit")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-fill-drip")
                .exposed()
                .with_description(
                    "How much new paint to add vs. smear existing color. 0% smudges without adding paint; 100% paints normally. In between, the brush picks up the canvas color and tints it with your paint.",
                ),
            PortDef::input("wetness", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Wetness")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-water")
                .exposed()
                .with_description(
                    "How strongly each brush touch leaves a mark. 0% leaves nothing; 100% applies the brush at full strength.",
                ),
            PortDef::input("opacity", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0)
                .with_natural_range(0.0, 1.0)
                .with_label("Opacity")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-droplet")
                .exposed()
                .with_description(
                    "Overall stroke strength. Lower values make the brush lighter.",
                ),
            PortDef::input("brush_preview", BrushWireType::Texture)
                .with_description("Brush shape shown under the cursor on hover"),
        ],
        params: &[],
        is_gpu: true,
        },
    }
}

pub struct WatercolorEvaluator;

impl BrushNodeEvaluator for WatercolorEvaluator {
    /// Watercolor commits ignore `gpu.blend_mode` — erase on a wet smudge
    /// brush isn't meaningful. The brush-tool UI hides the erase toggle.
    fn supports_erase(&self) -> bool {
        false
    }

    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        // GPU node — CPU evaluation is a no-op.
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let dab_handle = match ctx.input("dab") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let dab_size = ctx.input("dab_size").as_vec2();
        let position = ctx.input("position").as_vec2();
        let color = ctx.input("color").as_color();
        let deposit = ctx.input_f32("deposit").clamp(0.0, 1.0);
        let wetness = ctx.input_f32("wetness").clamp(0.0, 1.0);
        // Watercolor's commit is a direct blit (no source-over with pre_stroke
        // for an opacity cap), so opacity has to be applied per-dab via the
        // composite shader's `stroke_opacity` field.
        let opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);

        let dab_w = dab_size[0];
        let dab_h = dab_size[1];
        if dab_w <= 0.0 || dab_h <= 0.0 {
            return vec![];
        }

        // Layer-clip the dab footprint, push the canvas-space write bbox,
        // and snapshot the scratch under the dab into canvas_copy. Same
        // helper color_output and liquify use — None means the dab
        // doesn't overlap the layer (early-out).
        let half_w = dab_w * 0.5;
        let half_h = dab_h * 0.5;
        let footprint = match gpu.prepare_dab_canvas_copy(position, half_w, half_h) {
            Some(f) => f,
            None => return vec![],
        };
        let [pt_offset_x, pt_offset_y] = footprint.layer_offset;
        let [pt_width, pt_height] = footprint.layer_size;
        let [unclipped_x0, unclipped_y0] = footprint.unclipped_origin;
        let [x0, y0] = footprint.origin;
        let [quad_w, quad_h] = footprint.size;
        let x1 = x0 + quad_w;
        let y1 = y0 + quad_h;
        let [copy_canvas_x, copy_canvas_y] = footprint.copy_canvas_origin;

        // Dab UV mapping — identical to color_output. The dab content
        // occupies [0..dab_w]x[0..dab_h] within a (pool_w x pool_h) texture
        // that may be larger if the pool entry was sized up for reuse.
        let (pool_w, pool_h) = gpu.dab_pool.texture_size(dab_handle);
        let tex_w = pool_w as f32;
        let tex_h = pool_h as f32;
        let foot_w = dab_w;
        let foot_h = dab_h;
        let content_uv_w = dab_w / tex_w;
        let content_uv_h = dab_h / tex_h;
        let uv_min_x = (x0 - unclipped_x0) / foot_w * content_uv_w;
        let uv_min_y = (y0 - unclipped_y0) / foot_h * content_uv_h;
        let uv_max_x = (x1 - unclipped_x0) / foot_w * content_uv_w;
        let uv_max_y = (y1 - unclipped_y0) / foot_h * content_uv_h;

        // --- Pass 1: pickup (alpha-weighted average of canvas under brush) ---
        // Capture the read-mirror dimensions before the borrow chain that
        // follows ties up `gpu.scratch` for the rest of the function.
        let scratch_mirror_size = {
            let tex = gpu
                .scratch
                .as_deref()
                .expect("watercolor::evaluate_gpu requires Scratch")
                .read_mirror_texture();
            [tex.width() as f32, tex.height() as f32]
        };
        let pickup_uniforms = WatercolorPickupUniforms {
            center: position,
            copy_origin: [copy_canvas_x as f32, copy_canvas_y as f32],
            scratch_mirror_size,
            half_extent: [half_w.max(1.0), half_h.max(1.0)],
        };
        let pickup = gpu
            .pipelines
            .get::<WatercolorPickupPipeline>("watercolor_pickup");
        let pickup_offset = pickup.write_uniforms(gpu.queue, &pickup_uniforms);
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("watercolor::evaluate_gpu requires Scratch");
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-watercolor-pickup"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pickup.attachment_view(),
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, 1.0, 1.0, 0.0, 1.0);
            pass.set_pipeline(pickup.pipeline());
            pass.set_bind_group(0, pickup.uniform_bind_group(), &[pickup_offset]);
            pass.set_bind_group(1, scratch.read_mirror_bind_group(), &[]);
            pass.draw(0..3, 0..1);
        }

        // --- Pass 2: watercolor composite ---
        let composite_uniforms = WatercolorCompositeUniforms {
            paint_color: color,
            origin: [x0, y0],
            size: [quad_w, quad_h],
            target_offset: [pt_offset_x as f32, pt_offset_y as f32],
            target_size: [pt_width as f32, pt_height as f32],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            uv_min: [uv_min_x, uv_min_y],
            uv_max: [uv_max_x, uv_max_y],
            deposit,
            wetness,
            stroke_opacity: opacity,
            apply_selection: 1,
        };
        let composite = gpu
            .pipelines
            .get::<WatercolorCompositePipeline>("watercolor_composite");
        let composite_offset = composite.write_uniforms(gpu.queue, &composite_uniforms);
        let dab_bind_group = gpu.dab_pool.bind_group(dab_handle);
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-watercolor-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scratch.write_view(),
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, pt_width as f32, pt_height as f32, 0.0, 1.0);
            pass.set_pipeline(composite.pipeline());
            pass.set_bind_group(0, composite.uniform_bind_group(), &[composite_offset]);
            pass.set_bind_group(1, dab_bind_group, &[]);
            pass.set_bind_group(2, gpu.selection_bind_group, &[]);
            pass.set_bind_group(3, scratch.watercolor_sources_bind_group(), &[]);
            pass.draw(0..6, 0..1);
        }

        vec![]
    }

    /// Seed the stroke scratch with the immutable pre-stroke layer snapshot.
    /// `scratch read mirror` reads from the scratch, so seeding it with the layer's
    /// initial state lets every dab's pickup pass see real canvas pixels.
    /// Every rewind reseeds, discarding any in-stroke deposits.
    ///
    /// The copy spans the *full scratch texture*, not the canvas — both
    /// `pre_stroke_texture` and `stroke_scratch_texture` are sized to the
    /// paint target, which can exceed canvas dims for paste-extent or
    /// off-canvas-grown layers. Copying only canvas-sized would leave the
    /// off-canvas strip uninitialised, and `commit_scratch_blit` would
    /// then blit that transparent-black strip back over the layer.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(pre_stroke) = gpu.pre_stroke_texture else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
        let scratch_tex = scratch.write_texture();
        let w = scratch_tex.width();
        let h = scratch_tex.height();
        gpu.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: pre_stroke,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: scratch_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Direct blit scratch → layer — same shape as `liquify::commit`. The
    /// scratch already holds the finished image (pre_stroke + watercolor
    /// dabs source-over'd in place by `evaluate_gpu`), so commit just copies
    /// it across. `commit_scratch_blit` handles the format-aware path
    /// (hardware copy for RGBA8, render pass for R8 masks). `gpu.blend_mode`
    /// is ignored — erase semantics aren't meaningful for a smudge brush.
    fn commit(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
        paint_target.commit_scratch_blit(
            gpu.device,
            &mut gpu.encoder,
            gpu.pipelines,
            scratch.write_view(),
            scratch.write_texture(),
        );
    }

    /// Blit the upstream `brush_preview` texture into the overlay's preview
    /// mask — same shape as `color_output::render_preview`. Watercolor
    /// doesn't change what the cursor *looks* like; the preview shows the
    /// brush tip, not the deposit.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let preview_handle = match ctx.input("brush_preview") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let Some(target_view) = gpu.preview_mask_view else {
            return vec![];
        };
        let (target_w, target_h) = gpu.preview_mask_size;
        if target_w == 0 || target_h == 0 {
            return vec![];
        }
        let (extent_w, extent_h) = gpu.dab_pool.texture_size(preview_handle);
        if extent_w == 0 || extent_h == 0 {
            return vec![];
        }

        let uniforms = BlitUniforms {
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
        };
        let offset = gpu.pipelines.write_blit_uniforms(gpu.queue, &uniforms);
        let bg = gpu.dab_pool.bind_group(preview_handle).clone();

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("watercolor-render_preview"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(0.0, 0.0, target_w as f32, target_h as f32, 0.0, 1.0);
        pass.set_pipeline(gpu.pipelines.blit_pipeline());
        pass.set_bind_group(0, &gpu.pipelines.blit_uniform_bind_group, &[offset]);
        pass.set_bind_group(1, &bg, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);

        if gpu.brush_preview_info.is_none() {
            gpu.brush_preview_info = Some(BrushPreviewInfo {
                half_extent_canvas_px: [extent_w as f32 * 0.5, extent_h as f32 * 0.5],
                rotation_rad: 0.0,
            });
        }

        vec![]
    }
}
