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
use super::pipeline::BrushPipelines;
use super::scratch::Scratch;
use super::wire::TextureHandle;
use crate::gpu::paint_target::GpuPaintTarget;

/// Brush perf counters. Lives both per-`BrushGpuContext` (drained at
/// `submit_final`) and per-engine (accumulated across all contexts of a
/// stroke via [`AddAssign`]). The bench harness reads interval-deltas
/// against the engine-side accumulator via
/// [`crate::engine::BrushPerfDelta::between`].
///
/// `submit_us` is wall-clock around `queue.submit()`. The per-flush
/// counters describe workload (dab volume, union-bbox area), not host
/// time spent processing it. See `engine/perf.rs` for the design note.
#[derive(Default, Clone, Debug)]
pub struct BrushPerfCounters {
    /// Number of `place_dab` invocations during this counter's lifetime.
    /// Exposed by `engine.test_stroke_total_dabs()` for integration tests.
    pub dabs_placed: u32,
    /// Number of mid-stroke full-re-render fallbacks. Per-engine only —
    /// per-context value is always zero. Exposed by
    /// `engine.test_stroke_full_rerender_events()`.
    pub full_rerender_events: u32,
    /// Wall-clock microseconds inside `queue.submit()` (final + ring-flush).
    pub submit_us: u64,
    /// Number of `queue.submit()` calls.
    pub submits: u32,
    /// Number of dab-terminal flushes (one `flush_dabs` call each).
    pub dab_flushes: u32,
    /// Total dabs that flowed through a dab-batching terminal.
    pub flushed_dabs: u32,
    /// Sum of `union_w * union_h` across every dab flush.
    pub dab_union_bbox_area: u64,
    /// Per-flush dab counts. One entry per `flush_dabs` call. Drained
    /// per-event by the bench harness; production paths never read this.
    pub dabs_per_flush: Vec<u32>,
    /// Per-flush `union_w * union_h` in canvas pixels. Parallel to
    /// `dabs_per_flush`.
    pub dab_union_bbox_area_per_flush: Vec<u32>,
}

impl BrushPerfCounters {
    /// Increment the per-stroke dab counter. Called once per `place_dab`.
    pub fn record_dab(&mut self) {
        self.dabs_placed = self.dabs_placed.saturating_add(1);
    }

    /// Increment dab + dispatch counts at flush time once the queued dabs
    /// are about to be dispatched.
    pub fn record_dab_flush(&mut self, dab_count: u32) {
        self.flushed_dabs = self.flushed_dabs.saturating_add(dab_count);
        self.dab_flushes = self.dab_flushes.saturating_add(1);
    }

    /// Record the workload shape of one dab flush: `dab_count` queued
    /// dabs covering a `union_w × union_h` bbox in canvas pixels.
    /// Appends to both per-flush vectors and accumulates the area
    /// total. Called at the top of `flush_dabs` once the union bbox
    /// has been computed.
    pub fn record_dab_flush_workload(&mut self, dab_count: u32, union_w: u32, union_h: u32) {
        let area = (union_w as u64).saturating_mul(union_h as u64);
        self.dab_union_bbox_area = self.dab_union_bbox_area.saturating_add(area);
        self.dabs_per_flush.push(dab_count);
        self.dab_union_bbox_area_per_flush
            .push(area.min(u32::MAX as u64) as u32);
    }
}

impl std::ops::AddAssign for BrushPerfCounters {
    /// Merge per-context counters into the engine-side stroke accumulator
    /// (or any two accumulators in general). Scalars saturating-add;
    /// per-flush vectors append in order. `full_rerender_events` is
    /// per-engine state; per-context counters always contribute zero.
    fn add_assign(&mut self, mut rhs: Self) {
        self.dabs_placed = self.dabs_placed.saturating_add(rhs.dabs_placed);
        self.full_rerender_events = self
            .full_rerender_events
            .saturating_add(rhs.full_rerender_events);
        self.submit_us = self.submit_us.saturating_add(rhs.submit_us);
        self.submits = self.submits.saturating_add(rhs.submits);
        self.dab_flushes = self.dab_flushes.saturating_add(rhs.dab_flushes);
        self.flushed_dabs = self.flushed_dabs.saturating_add(rhs.flushed_dabs);
        self.dab_union_bbox_area = self
            .dab_union_bbox_area
            .saturating_add(rhs.dab_union_bbox_area);
        self.dabs_per_flush.append(&mut rhs.dabs_per_flush);
        self.dab_union_bbox_area_per_flush
            .append(&mut rhs.dab_union_bbox_area_per_flush);
    }
}

/// Hard cap on dab records that can be queued in a single phase across
/// any dab-batching terminal. Sized so the per-phase dab buffer is
/// trivial VRAM cost (16384 records × ~32-byte typical record ≈ 512
/// KB) and well above what any realistic stroke phase will reach
/// (~30 dabs even at high stabilisation). `queue_dab` debug-asserts
/// on this — overflow panics loudly in test/dev so the constant gets
/// bumped rather than silently truncating in release.
pub const MAX_DABS_PER_PHASE: u32 = 16384;

/// One queued dab waiting for the next `paint` terminal flush.
///
/// Layout MUST match the `Dab` struct in `shaders/brush/paint.wgsl` —
/// the WGSL binding reads this verbatim as a storage buffer element.
/// In std430, `vec2` is 8-byte aligned and `vec4` is 16-byte aligned,
/// so `vec2 pos + f32 radius + f32 softness + vec4 color` packs cleanly
/// into 32 bytes with no trailing pad.
///
/// Queued via `BrushGpuContext::queue_dab` into the shared byte-buffer
/// `pending_dab_bytes`. Other dab-batching terminals (watercolor_batched)
/// push their own record types into the same buffer — the brush graph
/// has at most one dab-batching terminal at a time, so the bytes are
/// unambiguous.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PaintDabRecord {
    /// Pen tip in **canvas pixels** (subtracted from layer offset in shader).
    pub pos: [f32; 2],
    /// Disc radius in canvas pixels (dab covers `pos ± radius`).
    pub radius: f32,
    /// Edge softness as fraction of radius. 0 = hard, 1 = fully feathered.
    pub softness: f32,
    /// Premultiplied paint color (rgba). Flow already multiplied in upstream.
    pub color: [f32; 4],
}

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

    /// Host-side counters for this context's lifetime. Written by the
    /// stroke engine + compute terminals via `record_*` helpers; drained
    /// by `submit_final` so the engine can `+= ` the result into its own
    /// stroke-level accumulator.
    pub perf: BrushPerfCounters,

    /// Dabs queued by whichever dab-batching terminal is active in the
    /// graph during a single pen event. Drained by that terminal's
    /// `flush_dabs` hook (one render pass for `paint`; two render
    /// passes — pickup atlas + composite — for `watercolor_batched`).
    /// The bytes are written by `bytemuck::bytes_of` on each terminal's
    /// own record struct — the WGSL binding reinterprets them as that
    /// terminal's `Dab` type. A brush graph has at most one
    /// dab-batching terminal at a time, so the bytes are unambiguous.
    ///
    /// Empty for brushes that don't use a dab-batching terminal.
    pub pending_dab_bytes: Vec<u8>,
    /// Number of dab records currently in `pending_dab_bytes`.
    /// Each terminal's record size is constant per terminal, so
    /// `bytes.len() == count * sizeof(Record)`; the count is tracked
    /// explicitly so flush code doesn't need to know the record size.
    pub pending_dab_count: u32,
    /// Layer-local bounding box covered by the queued dabs, as
    /// `[x0, y0, x1, y1]`. The terminal's `flush_dabs` reads it as a
    /// workload metric (recorded into `BrushPerfCounters` for the bench
    /// harness). `None` when the queue is empty. Per-flush `flush_dabs`
    /// implementations may also use it for a discriminator-or-clip
    /// decision, but neither shipped terminal does today —
    /// hardware-blend writes scale per-fragment, not per-bbox-pixel.
    pub pending_dabs_bbox: Option<[u32; 4]>,
}

impl<'a> BrushGpuContext<'a> {
    /// Submit the batched encoder and consume the context.
    ///
    /// All dab render passes in this batch are submitted in a single
    /// `queue.submit()` call — no per-dab submission needed thanks to
    /// dynamic uniform buffer offsets.
    ///
    /// Returns the per-context perf counters so the caller can `+=` them
    /// into the engine's stroke-level accumulator. The final submit's
    /// wall clock is folded into `submit_us` before returning.
    pub fn submit_final(mut self) -> BrushPerfCounters {
        let t = web_time::Instant::now();
        self.queue.submit([self.encoder.finish()]);
        let us = t.elapsed().as_micros() as u64;
        self.perf.submit_us = self.perf.submit_us.saturating_add(us);
        self.perf.submits = self.perf.submits.saturating_add(1);
        self.perf
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
            let t = web_time::Instant::now();
            let finished = std::mem::replace(
                &mut self.encoder,
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("brush-ring-flush"),
                    }),
            );
            self.queue.submit([finished.finish()]);
            self.pipelines.reset_uniform_rings();
            self.perf.submit_us = self
                .perf
                .submit_us
                .saturating_add(t.elapsed().as_micros() as u64);
            self.perf.submits = self.perf.submits.saturating_add(1);
        }
    }

    /// Queue a dab record into the shared byte buffer. The terminal
    /// that owns `flush_dabs` reinterprets the bytes as its own record
    /// type — see [`PaintDabRecord`] for the `paint` layout. The active
    /// brush has exactly one dab-batching terminal in its graph, so the
    /// bytes are unambiguous at flush time.
    pub fn queue_dab<T: bytemuck::Pod>(&mut self, record: &T) {
        self.pending_dab_bytes
            .extend_from_slice(bytemuck::bytes_of(record));
        self.pending_dab_count = self.pending_dab_count.saturating_add(1);
        debug_assert!(
            self.pending_dab_count <= MAX_DABS_PER_PHASE,
            "dab queue overflowed MAX_DABS_PER_PHASE ({MAX_DABS_PER_PHASE}); \
             bump the constant or flush more often",
        );
    }

    /// Drain the compute-dab queue. Returns the raw bytes (caller
    /// reinterprets via `bytemuck::cast_slice`) and the dab count. Also
    /// clears `pending_dabs_bbox`. Called from a terminal's
    /// `flush_dabs` hook once the dispatch is encoded.
    pub fn take_pending_dabs(&mut self) -> (Vec<u8>, u32) {
        let bytes = std::mem::take(&mut self.pending_dab_bytes);
        let count = std::mem::take(&mut self.pending_dab_count);
        self.pending_dabs_bbox = None;
        (bytes, count)
    }

    /// Discard the compute-dab queue without dispatching. Used at stroke
    /// begin / rewind to drop any state from a prior context, and by
    /// `flush_dabs` when its early-out path runs (empty queue, no
    /// scratch, etc.) so a follow-up dispatch doesn't see stale state.
    pub fn clear_pending_dabs(&mut self) {
        self.pending_dab_bytes.clear();
        self.pending_dab_count = 0;
        self.pending_dabs_bbox = None;
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
        self.prepare_dab_canvas_copy_split(position, half_w, half_h, half_w, half_h)
    }

    /// Generalization of [`Self::prepare_dab_canvas_copy`] that lets callers
    /// pass distinct write and read half-extents. The write region is the
    /// dab footprint (`position ± write_half`); the read region is the
    /// scratch-mirror snapshot footprint (`position ± read_half`). Read
    /// must be at least as large as write, but a brush that samples the
    /// scratch at an offset (smudge: per-dab `−motion`; clone: a stroke-
    /// scoped anchor) sizes the read region wider so the offset sample
    /// always lies inside the snapshot.
    ///
    /// The returned `DabFootprint`'s `origin/size` describe the write
    /// region (so the brush's render-pass viewport covers exactly the
    /// dab footprint); `copy_canvas_origin/copy_local_origin/copy_size`
    /// describe the (larger) read region, matching the read-mirror copy
    /// just issued.
    pub fn prepare_dab_canvas_copy_split(
        &mut self,
        position: [f32; 2],
        write_half_w: f32,
        write_half_h: f32,
        read_half_w: f32,
        read_half_h: f32,
    ) -> Option<DabFootprint> {
        debug_assert!(
            read_half_w >= write_half_w && read_half_h >= write_half_h,
            "read region must enclose write region",
        );
        let pt = self.paint_target.as_ref()?;
        let pt_canvas = pt.canvas_extent();

        let layer_x0 = pt_canvas.x0() as f32;
        let layer_y0 = pt_canvas.y0() as f32;
        let layer_x1 = layer_x0 + pt_canvas.width as f32;
        let layer_y1 = layer_y0 + pt_canvas.height as f32;

        // Write region (the dab footprint that the brush draws into).
        let unclipped_write_x0 = position[0] - write_half_w;
        let unclipped_write_y0 = position[1] - write_half_h;
        let write_x0 = unclipped_write_x0.max(layer_x0);
        let write_y0 = unclipped_write_y0.max(layer_y0);
        let write_x1 = (position[0] + write_half_w).min(layer_x1);
        let write_y1 = (position[1] + write_half_h).min(layer_y1);
        let quad_w = write_x1 - write_x0;
        let quad_h = write_y1 - write_y0;
        if quad_w <= 0.0 || quad_h <= 0.0 {
            return None;
        }

        // Read region (the scratch snapshot the brush samples from).
        let read_x0 = (position[0] - read_half_w).max(layer_x0);
        let read_y0 = (position[1] - read_half_h).max(layer_y0);
        let read_x1 = (position[0] + read_half_w).min(layer_x1);
        let read_y1 = (position[1] + read_half_h).min(layer_y1);

        // Floor-then-ceil so every fragment in the quad has a valid
        // scratch read mirror texel to read. `i32` keeps negative origins
        // (paste-extent layers, leftward-grown layers) representable.
        let copy_canvas_x = read_x0.floor() as i32;
        let copy_canvas_y = read_y0.floor() as i32;
        let copy_w = (read_x1.ceil() as i32 - copy_canvas_x) as u32;
        let copy_h = (read_y1.ceil() as i32 - copy_canvas_y) as u32;
        if copy_w == 0 || copy_h == 0 {
            return None;
        }

        // Save-point bbox tracks the write region — that's the only
        // damage to scratch. Canvas coords are stable across mid-stroke
        // layer growth (Storage Frame Rule).
        let write_bbox_x = write_x0.floor() as i32;
        let write_bbox_y = write_y0.floor() as i32;
        let write_bbox_w = (write_x1.ceil() as i32 - write_bbox_x) as u32;
        let write_bbox_h = (write_y1.ceil() as i32 - write_bbox_y) as u32;
        self.push_dab_write_bbox(crate::coord::CanvasRect::from_xywh(
            write_bbox_x,
            write_bbox_y,
            write_bbox_w,
            write_bbox_h,
        ));

        // The read mirror is filled from the stroke scratch, which is
        // layer-sized and indexed in layer-local pixels — translate
        // before issuing the copy.
        let copy_local_x = (copy_canvas_x - pt_canvas.x0()) as u32;
        let copy_local_y = (copy_canvas_y - pt_canvas.y0()) as u32;
        self.sync_scratch_read_mirror(copy_local_x, copy_local_y, copy_w, copy_h);

        Some(DabFootprint {
            layer_offset: [pt_canvas.x0(), pt_canvas.y0()],
            layer_size: [pt_canvas.width, pt_canvas.height],
            unclipped_origin: [unclipped_write_x0, unclipped_write_y0],
            origin: [write_x0, write_y0],
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
