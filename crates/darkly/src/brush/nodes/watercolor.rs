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

use crate::brush::eval::{BrushNodeEvaluator, BrushPreviewInfo, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipelines::{
    BlitUniforms, WatercolorCompositeUniforms, WatercolorPickupUniforms,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
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
    }
}

pub struct WatercolorEvaluator;

impl BrushNodeEvaluator for WatercolorEvaluator {
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
        let pickup_offset = gpu
            .pipelines
            .write_watercolor_pickup_uniforms(gpu.queue, &pickup_uniforms);
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("watercolor::evaluate_gpu requires Scratch");
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-watercolor-pickup"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: gpu.pipelines.watercolor_pickup_attachment_view(),
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
            pass.set_pipeline(gpu.pipelines.watercolor_pickup_pipeline());
            pass.set_bind_group(
                0,
                &gpu.pipelines.watercolor_pickup_uniform_bind_group,
                &[pickup_offset],
            );
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
        let composite_offset = gpu
            .pipelines
            .write_watercolor_composite_uniforms(gpu.queue, &composite_uniforms);
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
            pass.set_pipeline(gpu.pipelines.watercolor_composite_pipeline());
            pass.set_bind_group(
                0,
                &gpu.pipelines.watercolor_composite_uniform_bind_group,
                &[composite_offset],
            );
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
