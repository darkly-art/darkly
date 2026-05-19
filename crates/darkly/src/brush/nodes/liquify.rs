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
//!   current warp state into `scratch read mirror`, then the liquify shader reads
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

use crate::brush::dab_pool::DAB_REFERENCE_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, BrushPreviewInfo, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipelines::{BlitUniforms, CircleUniforms, LiquifyUniforms};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "liquify",
        category: "output",
        display_name: "Liquify",
        ports: vec![
            PortDef::input("size", BrushWireType::Scalar)
                .with_range(0.0, 4.0, 0.3)
                .with_natural_range(0.0, 4.0)
                .with_label("Size")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                .exposed()
                .with_description("Brush size. Can go above 100% for large-area warps (capped at 400%)."),
            PortDef::input("strength", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Strength")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-gauge-high")
                .exposed()
                .with_description("How far pixels are pushed by each brush touch"),
            PortDef::input("softness", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Softness")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-wave-square")
                .exposed()
                .with_description("Edge shape. Low values concentrate the warp at the brush center; high values spread it evenly across the brush."),
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Where to apply the warp"),
            // No `natural_range`: radians are a unit, not a normalized
            // signal. `pen.drawing_angle → direction` (the canonical
            // wire) is a unit-preserving identity.
            PortDef::input("direction", BrushWireType::Scalar)
                .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                .with_description("Direction to push pixels"),
            PortDef::input("distance", BrushWireType::Scalar)
                .with_description("How far the pen has traveled along the stroke"),
            PortDef::output("dab_size", BrushWireType::Vec2)
                .with_description("Size of the affected area"),
        ],
        params: &[],
        is_gpu: true,
    }
}

pub struct LiquifyEvaluator;

impl BrushNodeEvaluator for LiquifyEvaluator {
    /// Liquify warps pixels; `gpu.blend_mode` (paint/erase) has no
    /// meaning here. The brush-tool UI hides the erase toggle.
    fn supports_erase(&self) -> bool {
        false
    }

    /// Publish `dab_size` so the stroke engine can space dabs along the
    /// path. `size = 1.0` maps to diameter `DAB_REFERENCE_SIZE`; larger values
    /// allow brushes wider than that for full-canvas warp effects.
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let size = ctx.input_f32("size").clamp(0.0, 4.0);
        let diameter = (size * DAB_REFERENCE_SIZE as f32).max(1.0);
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
        let size = ctx.input_f32("size").clamp(0.0, 4.0);
        let strength = ctx.input_f32("strength").clamp(0.0, 1.0);
        let softness = ctx.input_f32("softness").clamp(0.0, 1.0);
        let position = ctx.input("position").as_vec2();
        let direction = ctx.input_f32("direction");
        let distance = ctx.input_f32("distance");

        let radius = size * (DAB_REFERENCE_SIZE as f32) * 0.5;
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

        // Layer-clip the dab footprint, push the canvas-space write bbox,
        // and snapshot the scratch under the disc into canvas_copy.
        // Subsequent dabs in the same place see the prior dab's warp via
        // the cached snapshot. `half` includes displacement padding so
        // the bilinear-sampled canvas_copy footprint always lies inside
        // the copied region. None means the disc doesn't overlap the
        // layer (early-out).
        let half = radius + displacement;
        let footprint = match gpu.prepare_dab_canvas_copy(position, half, half) {
            Some(f) => f,
            None => return vec![],
        };
        let [pt_offset_x, pt_offset_y] = footprint.layer_offset;
        let [pt_width, pt_height] = footprint.layer_size;
        let [x0, y0] = footprint.origin;
        let [rect_w, rect_h] = footprint.size;
        let [copy_local_x, copy_local_y] = footprint.copy_local_origin;

        // Direction → unit vector. First dab of a stroke has no prior
        // position and arrives here with `direction = 0` (east). Acceptable
        // at stroke onset; subsequent dabs use the actual direction.
        let dir_vec = [direction.cos(), direction.sin()];

        let uniforms = LiquifyUniforms {
            rect_origin: [x0, y0],
            rect_size: [rect_w, rect_h],
            target_offset: [pt_offset_x as f32, pt_offset_y as f32],
            target_size: [pt_width as f32, pt_height as f32],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            copy_origin: [copy_local_x as f32, copy_local_y as f32],
            center: position,
            direction: dir_vec,
            displacement,
            radius,
            softness,
            _pad: 0.0,
        };
        let offset = gpu.pipelines.write_liquify_uniforms(gpu.queue, &uniforms);
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("liquify::evaluate_gpu requires Scratch");

        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-liquify"),
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
            // Viewport must be the full layer (not the canvas) so the
            // shader can write into off-canvas pixels of paste-extent /
            // grown layers.
            pass.set_viewport(0.0, 0.0, pt_width as f32, pt_height as f32, 0.0, 1.0);
            pass.set_pipeline(gpu.pipelines.liquify_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.liquify_uniform_bind_group, &[offset]);
            pass.set_bind_group(1, gpu.selection_bind_group, &[]);
            pass.set_bind_group(2, scratch.read_mirror_bind_group(), &[]);
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
        let Some(pre_stroke) = gpu.pre_stroke_texture else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
        // Copy the full scratch — paste-extent or off-canvas-grown layers
        // size pre_stroke and scratch beyond canvas dims; copying only
        // canvas-sized would leave the off-canvas strip uninitialised,
        // and `commit_scratch_blit` would blit transparent-black back
        // over the layer.
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

    /// Replace the layer with the warped scratch. Straight copy, no blend —
    /// the scratch already holds the finished image because warp dabs
    /// produced the full canvas state (pre-stroke + displacement) in place.
    /// `commit_scratch_blit` on the paint target does the format-aware path:
    /// hardware copy for RGBA8 layer destinations, render pass for R8 mask
    /// destinations.
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

    /// Generate a soft circle preview at the brush's canvas-pixel size and
    /// blit it into the overlay preview mask. Reuses the existing circle
    /// pipeline — no liquify-specific preview shader needed.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(target_view) = gpu.preview_mask_view else {
            return vec![];
        };
        let (target_w, target_h) = gpu.preview_mask_size;
        if target_w == 0 || target_h == 0 {
            return vec![];
        }

        let size = ctx.input_f32("size").clamp(0.0, 4.0);
        // Soft edge for the preview ring — independent of the `softness`
        // waveshape knob (which controls warp falloff, not visual hardness).
        let preview_softness = 0.3_f32;
        let diameter_px = ((size * DAB_REFERENCE_SIZE as f32).max(4.0)) as u32;

        // Render the circle mask into a dab pool texture sized to the brush
        // extent. `acquire_sized` makes the texture self-describe its size,
        // so we don't need a separate size-reporting channel.
        let handle = gpu
            .dab_pool
            .acquire_sized(gpu.device, diameter_px, diameter_px);
        let circle_view = gpu.dab_pool.view(handle);
        // Liquify's preview ring is a plain hard-edged disc — algorithm = 0
        // (sine harmonic) with amplitude = 0 produces the unmodulated unit
        // circle, and the centroid lands at the texture centre by construction.
        let circle_uniforms = CircleUniforms {
            softness: preview_softness,
            algorithm: 0,
            amplitude: 0.0,
            frequency: 1.0,
            phase: 0.0,
            persistence: 0.5,
            seed: 0.0,
            octaves: 1,
            n1: 1.0,
            n2: 1.0,
            n3: 1.0,
            base_radius: 0.498,
            centroid_x: 0.0,
            centroid_y: 0.0,
            _pad: [0.0; 2],
        };
        let circle_offset = gpu
            .pipelines
            .write_circle_uniforms(gpu.queue, &circle_uniforms);
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
            pass.set_bind_group(
                0,
                &gpu.pipelines.circle_uniform_bind_group,
                &[circle_offset],
            );
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
