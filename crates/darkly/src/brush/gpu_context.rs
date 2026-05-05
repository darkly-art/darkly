//! GPU context bundle passed to brush node evaluators during `execute_gpu`
//! and `render_preview_pipeline`.
//!
//! Provides everything a GPU node needs: command encoder, device, queue,
//! dab texture pool, pipelines, canvas target, and selection bind group.
//! Stroke and preview modes are differentiated by *which* method the runner
//! invokes (`evaluate_gpu` vs `render_preview`), not by a flag on this
//! struct — terminals stop branching on a mode enum.

use std::collections::HashMap;

use super::dab_pool::DabTexturePool;
use super::eval::BrushPreviewInfo;
use super::pipelines::BrushPipelines;
use super::scratch::Scratch;
use super::wire::TextureHandle;
use crate::gpu::paint_target::GpuPaintTarget;

/// Everything a GPU brush node needs to record render passes.
///
/// Created once per rendering batch (per-segment in divergence, per-frame
/// in the no-divergence tail) and passed to the stroke engine.  Each dab
/// records its render passes into the encoder.  Dynamic uniform buffer
/// offsets allow all dabs to share one encoder without per-dab submission.
/// Call `submit_final()` when the batch is complete.
pub struct BrushGpuContext<'a> {
    pub encoder: wgpu::CommandEncoder,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub dab_pool: &'a mut DabTexturePool,
    pub pipelines: &'a BrushPipelines,
    /// The stroke scratch (write side + R/W-hazard read mirror).
    /// `Some` during stroke evaluation and palette-thumbnail rendering;
    /// `None` in cursor-preview mode where only `render_preview_pipeline`
    /// runs (no scratch is needed because the preview writes to
    /// `preview_mask_view` instead).
    ///
    /// Held mutably so `prepare_dab_canvas_copy` can lazy-grow the read
    /// mirror to fit the current dab's footprint.
    pub scratch: Option<&'a mut Scratch>,
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// The paint target the terminal is committing to: a layer (RGBA8) or
    /// mask (R8). `None` in preview mode (no commit happens).
    ///
    /// Replaces the loose `layer_view` / `layer_texture` / `layer_width` /
    /// `layer_height` / `layer_offset_x` / `layer_offset_y` fields. All those
    /// values are now `gpu.paint_target.X`. Format awareness lives in
    /// `GpuPaintTarget`'s brush extension (`commit_brush_dab`,
    /// `save_pre_stroke_snapshot`, `commit_scratch_blit`) — terminals call
    /// uniform methods on the paint target and never branch on R8 vs RGBA8.
    pub paint_target: Option<GpuPaintTarget<'a>>,
    /// Selection mask bind group (or default 1x1 white when no selection).
    pub selection_bind_group: &'a wgpu::BindGroup,
    /// In cursor-preview mode where `scratch` is `None`, this is the
    /// preview render target view that terminals' `render_preview` hooks
    /// write into.  Aliased only for codepaths that need *some* texture
    /// view (e.g. early-out checks).  `None` in stroke mode.
    pub preview_target_view: Option<&'a wgpu::TextureView>,
    /// Resource name → TextureHandle for images uploaded by the brush loader.
    /// Image nodes read from this to resolve their `resource_name` param.
    pub resource_handles: &'a HashMap<String, TextureHandle>,
    /// Composite blend mode override: 0 = source-over (paint), 1 = destination-out (erase).
    /// Set per-stroke by the engine based on the active tool.
    pub blend_mode: u32,
    /// Preview mask target. Populated by the engine during preview regen;
    /// terminal `render_preview` hooks blit their preview texture into it.
    /// `None` during stroke evaluation (the preview path isn't running).
    pub preview_mask_view: Option<&'a wgpu::TextureView>,
    pub preview_mask_size: (u32, u32),
    /// Set by a terminal's `render_preview` hook to publish overlay
    /// placement info (extent + rotation) to the engine. The engine reads
    /// this after `render_preview_pipeline` returns. `None` outside the
    /// preview path; first-write-wins if multiple terminals try to publish
    /// (unusual — typically one terminal owns the preview).
    pub brush_preview_info: Option<BrushPreviewInfo>,
    /// Pre-stroke layer snapshot. Supplied by `StrokeBuffer::save_pre_stroke`
    /// at the start of a stroke. `Some` during a stroke, `None` in preview.
    pub pre_stroke_texture: Option<&'a wgpu::Texture>,
    /// Bind group (canvas-copy BGL) over `pre_stroke_texture`, pre-built
    /// by `StrokeBuffer` so `color_output::commit` can bind it as the
    /// composite background without recreating bind groups every event.
    pub pre_stroke_bind_group: Option<&'a wgpu::BindGroup>,
    /// Union of canvas-pixel rects the current dab's passes write to. The
    /// node that issues the write is the only thing that knows the real
    /// footprint — stroke_engine can't derive it from `info.pos` because
    /// the graph may offset the dab (scatter, wobble, future
    /// position-modulating nodes). Each pass unions its rect into this via
    /// `push_dab_write_bbox`; stroke_engine reads it after `execute_gpu`
    /// for the save-point bbox and resets it before the next dab. `None`
    /// outside stroke evaluation.
    pub dab_write_canvas_bbox: Option<crate::coord::CanvasRect>,
}

impl<'a> BrushGpuContext<'a> {
    /// Submit the batched encoder and consume the context.
    ///
    /// All dab render passes in this batch are submitted in a single
    /// `queue.submit()` call — no per-dab submission needed thanks to
    /// dynamic uniform buffer offsets.
    pub fn submit_final(self) {
        self.queue.submit([self.encoder.finish()]);
    }

    /// Reset per-dab read-mirror state.  Called by the stroke engine
    /// before each dab so the first node that needs the read mirror this
    /// dab actually issues a fresh copy.  No-op in cursor-preview mode.
    pub fn reset_per_dab_read_cache(&mut self) {
        if let Some(scratch) = self.scratch.as_deref_mut() {
            scratch.reset_read_origin_cache();
        }
    }

    /// If any uniform ring is nearly full, submit the current encoder,
    /// reset all rings, and create a fresh encoder.  Called between dabs
    /// to prevent ring overflow — adds at most 1 extra submit per ~250
    /// dabs, which is negligible compared to the old per-dab submit.
    pub fn flush_if_needed(&mut self) {
        if self.pipelines.rings_nearly_full() {
            let finished = std::mem::replace(
                &mut self.encoder,
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("brush-ring-flush"),
                    }),
            );
            self.queue.submit([finished.finish()]);
            self.pipelines.reset_uniform_rings();
        }
    }

    /// Union a write-pass footprint into `dab_write_canvas_bbox`. Called by
    /// any GPU node whose pass writes to the stroke scratch, so
    /// stroke_engine can record a save-point bbox that matches what was
    /// actually drawn.
    pub fn push_dab_write_bbox(&mut self, bbox: crate::coord::CanvasRect) {
        if bbox.is_empty() {
            return;
        }
        self.dab_write_canvas_bbox = Some(match self.dab_write_canvas_bbox {
            Some(prev) => prev.union(bbox),
            None => bbox,
        });
    }

    /// Compute the layer-clipped per-dab footprint, push the canvas-space
    /// write bbox (so save_points / checkpoints cover the real damage
    /// region), and snapshot the scratch under the dab into
    /// `scratch read mirror`. Returns `None` if the dab footprint doesn't overlap
    /// the layer (early-out for the caller — typically `return vec![]`).
    ///
    /// Centralizes the canvas → layer-local translation that every brush
    /// terminal needs (color_output, watercolor, liquify). Getting it
    /// wrong manifests as strokes/warps shifted by `(offset_x, offset_y)`
    /// on grown / paste-extent layers — see the liquify regression in
    /// `tests/liquify.rs::warp_position_correct_on_offset_layer`.
    ///
    /// `half_w` / `half_h` are the dab's half-extent in canvas pixels,
    /// pre-clip. For a normal stamp dab pass `dab_w * 0.5` / `dab_h * 0.5`;
    /// for liquify pass `radius + displacement` (its disc plus the
    /// bilinear-sample padding).
    pub fn prepare_dab_canvas_copy(
        &mut self,
        position: [f32; 2],
        half_w: f32,
        half_h: f32,
    ) -> Option<DabFootprint> {
        let pt = self.paint_target.as_ref()?;
        let pt_offset_x = pt.offset_x;
        let pt_offset_y = pt.offset_y;
        let pt_width = pt.width;
        let pt_height = pt.height;

        let unclipped_x0 = position[0] - half_w;
        let unclipped_y0 = position[1] - half_h;
        let layer_x0 = pt_offset_x as f32;
        let layer_y0 = pt_offset_y as f32;
        let layer_x1 = layer_x0 + pt_width as f32;
        let layer_y1 = layer_y0 + pt_height as f32;
        let x0 = unclipped_x0.max(layer_x0);
        let y0 = unclipped_y0.max(layer_y0);
        let x1 = (position[0] + half_w).min(layer_x1);
        let y1 = (position[1] + half_h).min(layer_y1);

        let quad_w = x1 - x0;
        let quad_h = y1 - y0;
        if quad_w <= 0.0 || quad_h <= 0.0 {
            return None;
        }

        // Floor-then-ceil so every fragment in the quad has a valid
        // scratch read mirror texel to read. `i32` keeps negative origins
        // (paste-extent layers, leftward-grown layers) representable.
        let copy_canvas_x = x0.floor() as i32;
        let copy_canvas_y = y0.floor() as i32;
        let copy_w = (x1.ceil() as i32 - copy_canvas_x) as u32;
        let copy_h = (y1.ceil() as i32 - copy_canvas_y) as u32;
        if copy_w == 0 || copy_h == 0 {
            return None;
        }

        // Canvas coords are stable across mid-stroke layer growth
        // (Storage Frame Rule), so the bbox stored here remains valid
        // regardless of subsequent grow_layer events.
        self.push_dab_write_bbox(crate::coord::CanvasRect::from_xywh(
            copy_canvas_x,
            copy_canvas_y,
            copy_w,
            copy_h,
        ));

        // The read mirror is filled from the stroke scratch, which is
        // layer-sized and indexed in layer-local pixels — translate
        // before issuing the copy.
        let copy_local_x = (copy_canvas_x - pt_offset_x) as u32;
        let copy_local_y = (copy_canvas_y - pt_offset_y) as u32;
        self.sync_scratch_read_mirror(copy_local_x, copy_local_y, copy_w, copy_h);

        Some(DabFootprint {
            layer_offset: [pt_offset_x, pt_offset_y],
            layer_size: [pt_width, pt_height],
            unclipped_origin: [unclipped_x0, unclipped_y0],
            origin: [x0, y0],
            size: [quad_w, quad_h],
            copy_canvas_origin: [copy_canvas_x, copy_canvas_y],
            copy_local_origin: [copy_local_x, copy_local_y],
            copy_size: [copy_w, copy_h],
        })
    }

    /// Snapshot the stroke scratch under `(origin_x, origin_y, w, h)` into
    /// the read mirror at `(0, 0)`, lazy-growing the read mirror first if
    /// the requested footprint exceeds its current size.  Idempotent per
    /// dab: the first caller issues `copy_texture_to_texture`; subsequent
    /// callers with matching origin are no-ops.  Mismatched origins (or a
    /// grow) force a fresh copy.
    ///
    /// Both `smudge_stamp` (canvas sampling) and `color_output` (Porter-Duff
    /// bg) need this, and both compute the same footprint from the same
    /// position — the cache prevents a redundant copy per dab.
    ///
    /// No-op in cursor-preview mode (no scratch).
    pub fn sync_scratch_read_mirror(
        &mut self,
        origin_x: u32,
        origin_y: u32,
        width: u32,
        height: u32,
    ) {
        if let Some(scratch) = self.scratch.as_deref_mut() {
            scratch.sync_read_mirror(
                self.device,
                &mut self.encoder,
                origin_x,
                origin_y,
                width,
                height,
            );
        }
    }
}

/// Per-dab footprint produced by [`BrushGpuContext::prepare_dab_canvas_copy`].
///
/// Bundles every value brush terminals need to populate per-dab uniforms:
/// the layer-clipped quad in canvas coords, the layer-local origin of
/// the `scratch read mirror` snapshot the shader will read, and the layer's own
/// offset/size (for vertex NDC mapping against the layer-sized scratch
/// render target).
///
/// Coordinates are reported as `[x, y]` arrays so callers can name them
/// however reads best at the call site. `unclipped_origin` is the dab's
/// *pre-clip* top-left in canvas pixels — kept here because terminal
/// nodes that compute UVs for a stamp texture (color_output, watercolor)
/// derive `uv_min/uv_max` relative to the original (pre-clip) footprint.
#[derive(Copy, Clone, Debug)]
pub struct DabFootprint {
    /// `paint_target.offset_x/y` — layer's canvas-space offset.
    pub layer_offset: [i32; 2],
    /// `paint_target.width/height` — layer pixel dimensions.
    pub layer_size: [u32; 2],
    /// Dab footprint top-left in canvas pixels, *before* clipping to
    /// the layer extent.
    pub unclipped_origin: [f32; 2],
    /// Layer-clipped quad top-left in canvas pixels.
    pub origin: [f32; 2],
    /// Layer-clipped quad size in canvas pixels.
    pub size: [f32; 2],
    /// Integer canvas-space copy rect origin (`i32` — may be negative
    /// on paste-extent layers).
    pub copy_canvas_origin: [i32; 2],
    /// Layer-local origin of the `scratch read mirror` snapshot region (matches
    /// the `ensure_canvas_copy` source origin already issued). Use as
    /// the `copy_origin` uniform for shaders that read `scratch read mirror`.
    pub copy_local_origin: [u32; 2],
    /// `scratch read mirror` snapshot dimensions in pixels.
    pub copy_size: [u32; 2],
}
