//! Smudge GPU terminal node — drag pixels along the stroke.
//!
//! ## Stays on the dispatch path
//!
//! Smudge is one of the two builtin brushes (with [`liquify`]) that did
//! not migrate to the compiled-WGSL single-pass model. Its per-dab
//! semantics — read the scratch at `canvas_pos − motion`, blend, write
//! back to scratch — depend on each dab seeing the **cumulative** scratch
//! state mid-stroke. A single instanced render pass can't express that
//! feedback loop (every instance would read the same `pre_stroke`, not
//! the prior instance's output). Compiling the brush-mask half while
//! keeping the canvas read as a separate per-dab pass would still cost
//! one render pass per dab, so it isn't worth the framework complexity
//! for two brushes. See `handoff-port-everything-to-compiled.md`
//! §"What might not compile" for the design discussion.
//!
//! Per dab, the composite shader reads the scratch read mirror twice:
//! once at `canvas_pos − motion` (smear sample, what was under the brush
//! at the previous dab) and once at `canvas_pos` (current background).
//! It blends the two by `rate × mask × stroke_opacity × selection` and
//! writes back to scratch. Repeated dabs compound — each dab reads the
//! cumulatively-smeared scratch from the previous one — producing the
//! classic Photoshop / Krita smearing behaviour.
//!
//! Sibling of [`watercolor`] and [`liquify`]. Lifecycle follows liquify:
//!
//! - `begin_stroke` — copies `pre_stroke_texture` → `stroke_scratch` (full
//!   scratch, not canvas-sized: paste-extent / off-canvas-grown layers
//!   would otherwise have an uninitialised strip).
//! - `evaluate_gpu` (per dab) — one composite pass. The read region is
//!   sized to cover the dst rect AND the dst rect translated by
//!   `−motion`, via [`BrushGpuContext::prepare_dab_canvas_copy_split`].
//!   Stationary dabs (`|motion| < 0.5px`) short-circuit before the pass —
//!   the math is identity (`src == bg`) anyway, but skipping saves a
//!   GPU pass.
//! - `commit` — `commit_scratch_blit(scratch → layer)`, same shape as
//!   watercolor. `gpu.blend_mode` is intentionally ignored — "erase
//!   mode" on a smear isn't meaningful (erase removes pixels; smear
//!   moves them).
//! - `render_preview` — blits the upstream `brush_preview` texture into
//!   the overlay's preview mask.

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

// ── Pipeline ────────────────────────────────────────────────────────────

/// Uniform data for the smudge compositing shader.
///
/// Per-fragment, smudge reads the scratch read mirror twice — once at
/// `canvas_pos − motion` (the smear sample, the pixel that was under the
/// brush at the previous dab) and once at `canvas_pos` (the current
/// background, so source-over is correct where the dab mask falls off).
/// The read region is sized to cover the union of both, so `copy_origin`
/// here is *not* `floor(origin)` (as it is for watercolor's composite —
/// where the read region equals the write region) and must be carried
/// explicitly.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SmudgeCompositeUniforms {
    pub origin: [f32; 2],        // write quad top-left in canvas pixels
    pub size: [f32; 2],          // write quad size in canvas pixels
    pub target_offset: [f32; 2], // canvas-space offset of render target's (0,0) pixel
    pub target_size: [f32; 2],   // render target pixel dimensions (vertex NDC)
    pub canvas_size: [f32; 2],   // document canvas dimensions (fragment selection UV)
    pub uv_min: [f32; 2],        // min UV in dab texture
    pub uv_max: [f32; 2],        // max UV in dab texture
    pub motion: [f32; 2],        // per-dab delta from previous sample (canvas pixels)
    pub copy_origin: [f32; 2],   // canvas-space top-left of the scratch-mirror snapshot
    pub rate: f32,               // smudge rate (0 = dry, 1 = full smear)
    pub stroke_opacity: f32,     // per-stroke opacity cap (1.0 = no cap)
    pub apply_selection: u32,    // 1 = modulate by selection, 0 = ignore
    pub _pad: u32,
}

/// Smudge composite pipeline.  Single pass per dab — reads the scratch
/// read mirror twice (smear sample at `canvas_pos − motion`, background
/// at `canvas_pos`), blends, writes back to scratch.  Reuses
/// `canvas_copy_bgl` for the mirror binding (no pickup texture; no
/// extra binding needed).
pub struct SmudgeCompositePipeline {
    pipeline: wgpu::RenderPipeline,
    ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
}

impl SmudgeCompositePipeline {
    fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-smudge-composite"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../../shaders/brush/smudge.wgsl").into(),
                ),
            });
        // group(0) = uniforms, group(1) = dab, group(2) = selection,
        // group(3) = scratch read mirror (texture+sampler).
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-smudge-composite-layout"),
                bind_group_layouts: &[
                    ctx.uniform_bgl,
                    ctx.dab_bgl,
                    ctx.selection_bgl,
                    ctx.canvas_copy_bgl,
                ],
                immediate_size: 0,
            });
        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-smudge-composite"),
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
        let (ring, uniform_bind_group) = ctx.make_uniform_ring::<SmudgeCompositeUniforms>(
            "brush-smudge-composite-uniforms",
            "brush-smudge-composite-uniform-bg",
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

    pub fn write_uniforms(&self, queue: &wgpu::Queue, uniforms: &SmudgeCompositeUniforms) -> u32 {
        self.ring.write(queue, bytemuck::bytes_of(uniforms))
    }
}

impl BrushPipelineEntry for SmudgeCompositePipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.ring)
    }
}

fn smudge_composite_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "smudge_composite",
        build: |ctx| Box::new(SmudgeCompositePipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![smudge_composite_pipeline_reg()],
        node: NodeRegistration {
        type_id: "smudge",
        category: "output",
        display_name: "Smudge",
        ports: vec![
            PortDef::input("dab", BrushWireType::Texture)
                .with_description("Brush tip shape"),
            PortDef::input("dab_size", BrushWireType::Vec2)
                .with_description("Brush tip size in pixels"),
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Where to place the brush tip on the canvas"),
            PortDef::input("motion", BrushWireType::Vec2)
                .with_description("Per-dab motion vector — the offset to sample from"),
            PortDef::input("rate", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.6)
                .with_natural_range(0.0, 1.0)
                .with_label("Smudge")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-paint-roller")
                .exposed()
                .with_description(
                    "How strongly each touch drags the canvas along the stroke. \
                     Higher values produce a longer smear trail; lower values \
                     barely move pixels.",
                ),
            PortDef::input("opacity", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0)
                .with_natural_range(0.0, 1.0)
                .with_label("Opacity")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-droplet")
                .exposed()
                .with_description(
                    "Overall stroke strength. Lower values reduce how much the smudge affects the canvas.",
                ),
            PortDef::input("brush_preview", BrushWireType::Texture)
                .with_description("Brush shape shown under the cursor on hover"),
        ],
        params: &[],
        is_gpu: true,
        },
    }
}

pub struct SmudgeEvaluator;

impl BrushNodeEvaluator for SmudgeEvaluator {
    /// Erase mode (destination-out) on a smear isn't meaningful — erase
    /// removes pixels, smear moves them. `commit` ignores `gpu.blend_mode`
    /// accordingly; the brush-tool UI hides its erase toggle.
    fn supports_erase(&self) -> bool {
        false
    }

    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
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
        let motion = ctx.input("motion").as_vec2();
        let rate = ctx.input_f32("rate").clamp(0.0, 1.0);
        let opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);

        let dab_w = dab_size[0];
        let dab_h = dab_size[1];
        if dab_w <= 0.0 || dab_h <= 0.0 {
            return vec![];
        }

        // Stationary-dab early-out: with `motion == [0,0]` the shader's
        // `mix(bg, src, _)` collapses to `bg` (identity write). Skipping
        // the pass saves CPU/GPU work; correctness is unaffected. Also
        // covers the first-dab case (no previous sample → motion 0).
        if motion[0].abs() < 0.5 && motion[1].abs() < 0.5 {
            return vec![];
        }

        // Write region = dab footprint. Read region = dab footprint
        // expanded by `|motion|` in each axis so the smear sample at
        // `canvas_pos − motion` always lies inside the read-mirror
        // snapshot. Symmetric expansion wastes ~2× per axis vs. a
        // signed-motion tight fit, but keeps the math simple.
        let write_half_w = dab_w * 0.5;
        let write_half_h = dab_h * 0.5;
        let read_half_w = write_half_w + motion[0].abs().ceil();
        let read_half_h = write_half_h + motion[1].abs().ceil();
        let footprint = match gpu.prepare_dab_canvas_copy_split(
            position,
            write_half_w,
            write_half_h,
            read_half_w,
            read_half_h,
        ) {
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

        // Dab UV mapping — identical to color_output / watercolor. The dab
        // content occupies [0..dab_w] × [0..dab_h] within a (pool_w, pool_h)
        // texture that may be larger if the pool entry was sized up.
        let (pool_w, pool_h) = gpu.dab_pool.texture_size(dab_handle);
        let tex_w = pool_w as f32;
        let tex_h = pool_h as f32;
        let content_uv_w = dab_w / tex_w;
        let content_uv_h = dab_h / tex_h;
        let uv_min_x = (x0 - unclipped_x0) / dab_w * content_uv_w;
        let uv_min_y = (y0 - unclipped_y0) / dab_h * content_uv_h;
        let uv_max_x = (x1 - unclipped_x0) / dab_w * content_uv_w;
        let uv_max_y = (y1 - unclipped_y0) / dab_h * content_uv_h;

        let uniforms = SmudgeCompositeUniforms {
            origin: [x0, y0],
            size: [quad_w, quad_h],
            target_offset: [pt_offset_x as f32, pt_offset_y as f32],
            target_size: [pt_width as f32, pt_height as f32],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            uv_min: [uv_min_x, uv_min_y],
            uv_max: [uv_max_x, uv_max_y],
            motion,
            copy_origin: [copy_canvas_x as f32, copy_canvas_y as f32],
            rate,
            stroke_opacity: opacity,
            apply_selection: 1,
            _pad: 0,
        };
        let smudge = gpu
            .pipelines
            .get::<SmudgeCompositePipeline>("smudge_composite");
        let offset = smudge.write_uniforms(gpu.queue, &uniforms);
        let dab_bind_group = gpu.dab_pool.bind_group(dab_handle);
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("smudge::evaluate_gpu requires Scratch");

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("brush-smudge-composite"),
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
        pass.set_pipeline(smudge.pipeline());
        pass.set_bind_group(0, smudge.uniform_bind_group(), &[offset]);
        pass.set_bind_group(1, dab_bind_group, &[]);
        pass.set_bind_group(2, gpu.selection_bind_group, &[]);
        pass.set_bind_group(3, scratch.read_mirror_bind_group(), &[]);
        pass.draw(0..6, 0..1);

        vec![]
    }

    /// Seed the stroke scratch with the immutable pre-stroke layer
    /// snapshot, full scratch (not canvas-sized — paste-extent / off-
    /// canvas-grown layers would otherwise have an uninitialised strip).
    /// Same shape as `watercolor::begin_stroke` and `liquify::begin_stroke`.
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

    /// Direct blit scratch → layer. The scratch already holds the
    /// finished image (pre_stroke + smudge passes accumulated in place
    /// by `evaluate_gpu`), so commit just copies it across.
    /// `gpu.blend_mode` is ignored — erase semantics aren't meaningful
    /// for a smear (erase removes pixels; smear moves them).
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

    /// Blit upstream `brush_preview` into the overlay's preview mask —
    /// same shape as `watercolor::render_preview`. The hover preview
    /// shows the brush tip, not any smudge effect.
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
            label: Some("smudge-render_preview"),
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
