//! Liquify warp GPU terminal node.
//!
//! A warp brush that pushes pixels along the pen's drawing direction with a
//! radial falloff around the brush center. Unlike paint terminals it does
//! not deposit pigment — every pixel inside the brush disc is replaced with
//! a sample from elsewhere in the scratch, displaced opposite the direction
//! of travel.
//!
//! Displacement *magnitude* is a function of `strength` and `radius` only —
//! pen speed is intentionally ignored. A slow deliberate drag produces the
//! same per-dab displacement as a fast flick. Speed still governs stroke
//! length (dabs fire as the pen moves), but not per-dab warp intensity.
//!
//! ## Stroke lifecycle
//!
//! - `begin_stroke` — copies `pre_stroke_texture` → `stroke_scratch_texture`
//!   (full canvas). The scratch starts matching the layer so warp dabs have
//!   realistic pixels to read. On rewind boundaries this reseeds the scratch
//!   to the stable pre-stroke state, discarding all accumulated warps; the
//!   checkpoint system then overlays the bbox snapshot for partial rewinds.
//! - `evaluate_gpu` (per dab) — `ensure_canvas_copy` captures the scratch's
//!   current warp state into `canvas_copy`, then the liquify shader reads
//!   the copy at a displaced UV (`canvas_pos − direction × magnitude × falloff`)
//!   and writes the warped sample back into the scratch. Successive dabs
//!   read each other's output, so the warp compounds along the stroke.
//! - `commit` — `copy_texture_to_texture(scratch → layer)`, full canvas.
//!   The scratch already holds the finished warped canvas; no source-over.
//!   `gpu.blend_mode` is ignored — warping isn't paint.
//! - `render_preview` — synthesizes a soft circle mask sized to the brush
//!   radius using the existing circle pipeline, then blits it into the
//!   overlay's preview mask and publishes placement info. Liquify has no
//!   stamp upstream, so preview generation is internal.
//!
//! Compared to Krita's CPU-bound [`KisLiquifyTransformWorker`](krita/libs/image/kis_liquify_transform_worker.cpp#L442)
//! (per-pixel polygon rasterisation over a grid-of-points deformation),
//! this runs as one fragment pass per dab over the affected rect. Cost
//! scales with the brush footprint, not the canvas.
//!
//! The `softness` input is a waveshape knob (not an edge-softness slider):
//! 0 → spike (sharp peak), 0.4 → saw (linear), 0.5 → sine (smooth),
//! 1 → square (hard edge). See `liquify.wgsl` for the interpolation formula.

use crate::brush::dab_pool::MAX_DAB_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, BrushPreviewInfo, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::pipelines::{BlitUniforms, CircleUniforms, LiquifyUniforms};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "liquify",
        category: "gpu",
        display_name: "Liquify",
        ports: vec![
            PortDef::input("size", BrushWireType::Scalar)
                .with_range(0.0, 4.0, 0.3)
                .with_label("Size")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                .exposed()
                .with_description("Brush radius as a multiple of the base size (1.0 = 100% = half MAX_DAB_SIZE). Uncapped above 100% for large-area warps."),
            PortDef::input("strength", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_label("Strength")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-gauge-high")
                .exposed()
                .with_description("How far pixels are pushed per dab (as a fraction of motion)"),
            PortDef::input("softness", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_label("Softness")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-wave-square")
                .exposed()
                .with_description("Falloff waveshape: 0 = spike (sharp peak), 0.4 = saw, 0.5 = sine, 1 = square"),
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Brush center in canvas pixels"),
            PortDef::input("direction", BrushWireType::Scalar)
                .with_range(-6.2832, 6.2832, 0.0)
                .with_description("Warp direction in radians (typically wired from pen_input.drawing_angle). 0 = east."),
            PortDef::input("distance", BrushWireType::Scalar)
                .with_description("Cumulative pen travel in pixels (typically wired from pen_input.distance). Used as a 'has the pen moved yet' gate — the first dab of a stationary click has distance=0 and is skipped so liquify doesn't warp in a default direction before the stroke actually has one."),
            PortDef::output("dab_size", BrushWireType::Vec2)
                .with_description("Affected diameter in canvas pixels (used by stroke engine for dab spacing)"),
        ],
        params: &[],
        is_gpu: true,
    }
}

pub struct LiquifyEvaluator;

impl BrushNodeEvaluator for LiquifyEvaluator {
    /// Publish `dab_size` so the stroke engine can space dabs along the
    /// path. `size = 1.0` maps to diameter `MAX_DAB_SIZE`; larger values
    /// allow brushes wider than that for full-canvas warp effects.
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let size = ctx.input_f32("size").max(0.0);
        let diameter = (size * MAX_DAB_SIZE as f32).max(1.0);
        vec![("dab_size".into(), ScalarValue::Vec2([diameter, diameter]))]
    }

    /// Per-dab warp: read the scratch's current state (via canvas_copy) at
    /// a displaced UV, write the warped sample into the scratch's disc
    /// region. Outside the disc the shader discards, preserving LoadOp::Load
    /// content.
    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let size = ctx.input_f32("size").max(0.0);
        let strength = ctx.input_f32("strength").clamp(0.0, 1.0);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);
        let position = ctx.input("position").as_vec2();
        let direction = ctx.input_f32("direction");
        let distance = ctx.input_f32("distance");

        let radius = size * (MAX_DAB_SIZE as f32) * 0.5;
        if radius < 1.0 {
            return vec![];
        }

        if strength < 1e-4 {
            return vec![];
        }

        // Gate: skip dabs that have no direction of travel yet. The stroke
        // engine's first dab fires before any motion is recorded (pen-down
        // moment) — drawing_angle defaults to 0 (east), so without this gate
        // a stationary click would immediately warp rightward. `distance`
        // stays at zero until the pen actually moves, so it's the cleanest
        // "stroke has a direction" signal.
        if distance < 0.5 {
            return vec![];
        }

        // Per-dab displacement magnitude is a fixed fraction of the brush
        // radius — pen speed doesn't enter the equation. `DRAG_FACTOR` is
        // tuned so that strength=1 pushes pixels approximately one dab
        // spacing (~25% of radius) per dab, giving a 1:1 "drag" feel along
        // the stroke path. Tune empirically as users give feedback.
        const DRAG_FACTOR: f32 = 0.25;
        let displacement = radius * DRAG_FACTOR * strength;

        // Bounding box: disc + displacement padding so the bilinear-sampled
        // canvas_copy footprint is always inside the copied region.
        let half = radius + displacement;
        let unclipped_x0 = position[0] - half;
        let unclipped_y0 = position[1] - half;
        let x0 = unclipped_x0.max(0.0);
        let y0 = unclipped_y0.max(0.0);
        let x1 = (position[0] + half).min(gpu.canvas_width as f32);
        let y1 = (position[1] + half).min(gpu.canvas_height as f32);

        let rect_w = x1 - x0;
        let rect_h = y1 - y0;
        if rect_w <= 0.0 || rect_h <= 0.0 {
            return vec![];
        }

        // Integer copy rect — match composite.rs's floor-then-ceil pattern so
        // every fragment in the quad has a valid canvas_copy texel to read.
        let copy_x = x0.floor() as u32;
        let copy_y = y0.floor() as u32;
        let copy_w = ((x1.ceil() as u32).saturating_sub(copy_x))
            .min(gpu.canvas_width - copy_x);
        let copy_h = ((y1.ceil() as u32).saturating_sub(copy_y))
            .min(gpu.canvas_height - copy_y);
        if copy_w == 0 || copy_h == 0 {
            return vec![];
        }

        // Publish the footprint so save_points / checkpoints cover the real
        // damage region.
        gpu.push_dab_write_bbox([copy_x, copy_y, copy_w, copy_h]);

        // Snapshot the scratch under the disc into canvas_copy. Subsequent
        // dabs in the same place see the prior dab's warp.
        gpu.ensure_canvas_copy(copy_x, copy_y, copy_w, copy_h);

        // Direction → unit vector. First dab of a stroke has no prior
        // position and arrives here with `direction = 0` (east). Acceptable
        // at stroke onset; subsequent dabs use the actual direction.
        let dir_vec = [direction.cos(), direction.sin()];

        let uniforms = LiquifyUniforms {
            rect_origin: [x0, y0],
            rect_size: [rect_w, rect_h],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            copy_origin: [copy_x as f32, copy_y as f32],
            center: position,
            direction: dir_vec,
            displacement,
            radius,
            softness,
            _pad: 0.0,
        };
        let offset = gpu.pipelines.write_liquify_uniforms(gpu.queue, &uniforms);

        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-liquify"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: gpu.stroke_scratch_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(
                0.0, 0.0,
                gpu.canvas_width as f32, gpu.canvas_height as f32,
                0.0, 1.0,
            );
            pass.set_pipeline(gpu.pipelines.liquify_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.liquify_uniform_bind_group, &[offset]);
            pass.set_bind_group(1, gpu.selection_bind_group, &[]);
            pass.set_bind_group(2, &gpu.pipelines.canvas_copy_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }

        // dab_size is CPU-computed in evaluate_cpu; the slot is already set.
        // Returning empty avoids a redundant slot write during GPU eval.
        vec![]
    }

    /// Seed the stroke scratch with the immutable pre-stroke layer snapshot.
    /// Every rewind (full or partial-preamble) runs through this hook, so
    /// the scratch always restarts from the same pre-stroke state regardless
    /// of prior commits. Reading `layer_texture` instead would compound
    /// warps exponentially on each rewind.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(pre_stroke) = gpu.pre_stroke_texture else { return };
        let w = gpu.canvas_width;
        let h = gpu.canvas_height;
        gpu.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: pre_stroke,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: gpu.stroke_scratch_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Replace the layer with the warped scratch. Straight copy, no blend —
    /// the scratch already holds the finished image because warp dabs
    /// produced the full canvas state (pre-stroke + displacement) in place.
    fn commit(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(layer) = gpu.layer_texture else { return };
        let w = gpu.canvas_width;
        let h = gpu.canvas_height;
        gpu.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: gpu.stroke_scratch_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: layer,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Generate a soft circle preview at the brush's canvas-pixel size and
    /// blit it into the overlay preview mask. Reuses the existing circle
    /// pipeline — no liquify-specific preview shader needed.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(target_view) = gpu.preview_mask_view else { return vec![] };
        let (target_w, target_h) = gpu.preview_mask_size;
        if target_w == 0 || target_h == 0 {
            return vec![];
        }

        let size = ctx.input_f32("size").max(0.0);
        // Soft edge for the preview ring — independent of the `softness`
        // waveshape knob (which controls warp falloff, not visual hardness).
        let preview_softness = 0.3_f32;
        let diameter_px = ((size * MAX_DAB_SIZE as f32).max(4.0)) as u32;

        // Render the circle mask into a dab pool texture sized to the brush
        // extent. `acquire_sized` makes the texture self-describe its size,
        // so we don't need a separate size-reporting channel.
        let handle = gpu.dab_pool.acquire_sized(gpu.device, diameter_px, diameter_px);
        let circle_view = gpu.dab_pool.view(handle);
        let circle_uniforms = CircleUniforms {
            softness: preview_softness,
            _pad: [0.0; 3],
        };
        let circle_offset = gpu.pipelines.write_circle_uniforms(gpu.queue, &circle_uniforms);
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquify-preview-circle"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: circle_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(0.0, 0.0, diameter_px as f32, diameter_px as f32, 0.0, 1.0);
            pass.set_pipeline(gpu.pipelines.circle_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.circle_uniform_bind_group, &[circle_offset]);
            pass.draw(0..3, 0..1);
        }

        // Blit the circle into the overlay preview mask (fixed-size texture
        // the overlay samples for display).
        let blit_uniforms = BlitUniforms {
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
        };
        let blit_offset = gpu.pipelines.write_blit_uniforms(gpu.queue, &blit_uniforms);
        let circle_bg = gpu.dab_pool.bind_group(handle).clone();
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquify-preview-blit"),
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
            pass.set_bind_group(0, &gpu.pipelines.blit_uniform_bind_group, &[blit_offset]);
            pass.set_bind_group(1, &circle_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // Publish placement info — canvas-pixel half-extent for the overlay
        // primitive. First terminal to publish wins (there's only one here).
        if gpu.brush_preview_info.is_none() {
            let half = diameter_px as f32 * 0.5;
            gpu.brush_preview_info = Some(BrushPreviewInfo {
                half_extent_canvas_px: [half, half],
                rotation_rad: 0.0,
            });
        }

        vec![]
    }
}
