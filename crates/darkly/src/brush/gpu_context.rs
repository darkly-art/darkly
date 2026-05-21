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

/// Host-side wall-clock counters tracking per-dab cost inside a single
/// `BrushGpuContext` lifetime. Drained by `submit_final` so the engine
/// can fold them into the per-stroke summary.
///
/// All times are microseconds. None of these reflect GPU shader execution
/// — they're recorded around CPU-side encoder ops, IPC submissions, and
/// host-side bookkeeping. The lag investigation indicated drain dominated
/// the frame, so host-side timing is what we actually want.
///
/// Callers OUTSIDE this module (the stroke engine + individual brush node
/// evaluators) call `record_*` helpers to attribute their work to the
/// right bucket. INTERNAL helpers (`sync_scratch_read_mirror`,
/// `flush_if_needed`, `submit_final`) instrument themselves.
#[derive(Default, Clone, Copy, Debug)]
pub struct BrushPerfCounters {
    /// Number of `place_dab` invocations that ran during this context's
    /// lifetime.
    pub dabs_placed: u32,
    /// Sum of per-dab end-to-end host time (from `place_dab` start to end,
    /// inclusive of every node + pool + flush check).
    pub dab_total_us: u64,
    /// Sum of CPU node-graph evaluation time (`execute_cpu` +
    /// `execute_gpu` orchestration overhead — excludes the node
    /// evaluators' own GPU encoder work, which is bucketed by node type
    /// below).
    pub graph_eval_us: u64,
    /// Sum of host time recording stamp render passes
    /// (`stamp::encode_stamp_pass` — `begin_render_pass` + draw + drop).
    pub stamp_pass_us: u64,
    /// Sum of host time recording color_output composite render passes
    /// (the `begin_render_pass` + draw + drop sequence in
    /// `color_output::evaluate_gpu`).
    pub composite_pass_us: u64,
    /// Sum of host time spent issuing the read-mirror
    /// `copy_texture_to_texture` (`Scratch::sync_read_mirror`).
    pub read_mirror_copy_us: u64,
    /// Sum of host time spent in `DabTexturePool::acquire_sized`.
    pub pool_acquire_us: u64,
    /// Sum of host time spent in `flush_if_needed` when it actually
    /// flushes (excludes the no-op fast path).
    pub flush_submit_us: u64,
    /// Time spent inside the final `queue.submit()` in `submit_final`.
    pub submit_us: u64,
    /// Number of `queue.submit()` calls issued from this context (final
    /// submit + every mid-context flush).
    pub submits: u32,

    // --- Newer per-dab buckets (added to chase the unattributed 39µs/dab) ---
    /// Total host time inside `runner.execute_gpu(gpu)` per dab. This is
    /// a wrapper around all GPU-node evaluators; `stamp_pass_us` and
    /// `composite_pass_us` are subsets of it. The delta
    /// (`execute_gpu_us − stamp_pass_us − composite_pass_us −
    /// read_mirror_copy_us`) is framework overhead + other GPU-node
    /// evaluators.
    pub execute_gpu_us: u64,
    /// Sum of host time spent in `DabTexturePool::release_all` per dab.
    pub release_all_us: u64,
    /// Sum of host time spent in `place_dab`'s post-`execute_gpu`
    /// bookkeeping: terminal-output reads, canvas-bbox math, save-points
    /// push.
    pub post_dab_us: u64,

    /// Number of GPU steps the brush graph runner iterates over, summed
    /// across all dabs in this context. Divide by `dabs_placed` for the
    /// average GPU-step count per dab.
    pub gpu_steps: u64,
    /// Sum of host time inside `gather_inputs` across all GPU steps. The
    /// runner builds a fresh `HashMap` per step and walks `port_defs` for
    /// each input's wire-boundary remap; this counter tells us how much
    /// of the per-dab cost is that bookkeeping.
    pub gather_inputs_us: u64,
    /// Sum of host time inside the per-step output write-back loop.
    /// Iterates `step.output_slots` linearly with a string match per
    /// produced output.
    pub step_outputs_us: u64,
    /// Sum of host time inside `evaluator.evaluate_gpu(...)` calls — the
    /// evaluator-body time. Excludes the runner-framework time
    /// (`gather_inputs`, `step_outputs`, evaluator lookup, context build).
    /// Sub-buckets like `stamp_pass_us`, `composite_pass_us`,
    /// `read_mirror_copy_us`, `pool_acquire_us` are subsets of this.
    pub evaluate_gpu_call_us: u64,
    /// Sum of host time inside `evaluator.evaluate_cpu(...)` calls *made
    /// from inside `dispatch_gpu`* (promoted-CPU nodes that landed in the
    /// GPU phase because they depend on a GPU output). Separate from the
    /// `execute_cpu` phase's evaluation.
    pub evaluate_cpu_in_gpu_us: u64,

    // --- Inside-evaluator hotspots (added to chase the 30µs of
    //     `eval_gpu_call` time that the per-pass timers don't account
    //     for) ---
    /// Sum of host time inside `prepare_dab_canvas_copy` calls
    /// (`color_output::evaluate_gpu`). Includes the inner
    /// `sync_scratch_read_mirror` cost already tracked in
    /// `read_mirror_copy_us`; subtract to isolate the footprint-math
    /// + `push_dab_write_bbox` + `DabFootprint` build.
    pub prepare_canvas_copy_us: u64,
    /// Sum of host time inside `write_composite_uniforms` calls
    /// (`queue.write_buffer` + ring-slot math).
    pub write_composite_uniforms_us: u64,
    /// Sum of host time inside `write_stamp_uniforms` calls. Subset of
    /// `stamp_pass_us` (the stamp-pass timer wraps the whole
    /// `encode_stamp_pass` call which contains the uniform write).
    pub write_stamp_uniforms_us: u64,
    /// Sum of host time inside `ctx.input(...)` lookups performed at
    /// the top of `color_output::evaluate_gpu` (HashMap-by-string-key
    /// reads + `as_vec2`/match coercion).
    pub ctx_input_us: u64,
}

impl BrushPerfCounters {
    /// Record host wall-clock time attributed to CPU brush-graph eval. The
    /// stroke engine calls this around `execute_cpu` + `execute_gpu`
    /// minus already-attributed work (the node evaluators add to their
    /// own buckets directly).
    pub fn record_graph_eval(&mut self, us: u64) {
        self.graph_eval_us = self.graph_eval_us.saturating_add(us);
    }

    /// Record host wall-clock time for one stamp render pass. Called by
    /// the stamp node's evaluator.
    pub fn record_stamp_pass(&mut self, us: u64) {
        self.stamp_pass_us = self.stamp_pass_us.saturating_add(us);
    }

    /// Record host wall-clock time for one color_output composite pass.
    pub fn record_composite_pass(&mut self, us: u64) {
        self.composite_pass_us = self.composite_pass_us.saturating_add(us);
    }

    /// Record host wall-clock time for one `acquire_sized` call.
    pub fn record_pool_acquire(&mut self, us: u64) {
        self.pool_acquire_us = self.pool_acquire_us.saturating_add(us);
    }

    /// Increment the dab counter and add to the per-dab total. Called by
    /// `place_dab`.
    pub fn record_dab(&mut self, us: u64) {
        self.dabs_placed = self.dabs_placed.saturating_add(1);
        self.dab_total_us = self.dab_total_us.saturating_add(us);
    }

    /// Record host wall-clock time around `runner.execute_gpu(gpu)`. The
    /// node-specific buckets (`stamp_pass_us`, `composite_pass_us`,
    /// `read_mirror_copy_us`) are subsets — the delta is framework
    /// overhead + other GPU nodes.
    pub fn record_execute_gpu(&mut self, us: u64) {
        self.execute_gpu_us = self.execute_gpu_us.saturating_add(us);
    }

    /// Record host wall-clock time around `dab_pool.release_all()`.
    pub fn record_release_all(&mut self, us: u64) {
        self.release_all_us = self.release_all_us.saturating_add(us);
    }

    /// Record host wall-clock time around `place_dab`'s post-`execute_gpu`
    /// bookkeeping.
    pub fn record_post_dab(&mut self, us: u64) {
        self.post_dab_us = self.post_dab_us.saturating_add(us);
    }

    /// Increment the GPU step counter. Called once per step inside the
    /// brush-graph runner's `dispatch_gpu` loop.
    pub fn record_gpu_step(&mut self) {
        self.gpu_steps = self.gpu_steps.saturating_add(1);
    }

    /// Record host time spent in one `gather_inputs` call.
    pub fn record_gather_inputs(&mut self, us: u64) {
        self.gather_inputs_us = self.gather_inputs_us.saturating_add(us);
    }

    /// Record host time spent in one step's output write-back loop.
    pub fn record_step_outputs(&mut self, us: u64) {
        self.step_outputs_us = self.step_outputs_us.saturating_add(us);
    }

    /// Record host time spent inside one `evaluator.evaluate_gpu(...)`
    /// call.
    pub fn record_evaluate_gpu_call(&mut self, us: u64) {
        self.evaluate_gpu_call_us = self.evaluate_gpu_call_us.saturating_add(us);
    }

    /// Record host time spent inside one `evaluator.evaluate_cpu(...)`
    /// call dispatched from `dispatch_gpu`.
    pub fn record_evaluate_cpu_in_gpu(&mut self, us: u64) {
        self.evaluate_cpu_in_gpu_us = self.evaluate_cpu_in_gpu_us.saturating_add(us);
    }

    pub fn record_prepare_canvas_copy(&mut self, us: u64) {
        self.prepare_canvas_copy_us = self.prepare_canvas_copy_us.saturating_add(us);
    }

    pub fn record_write_composite_uniforms(&mut self, us: u64) {
        self.write_composite_uniforms_us = self.write_composite_uniforms_us.saturating_add(us);
    }

    pub fn record_write_stamp_uniforms(&mut self, us: u64) {
        self.write_stamp_uniforms_us = self.write_stamp_uniforms_us.saturating_add(us);
    }

    pub fn record_ctx_input(&mut self, us: u64) {
        self.ctx_input_us = self.ctx_input_us.saturating_add(us);
    }
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

    /// Host wall-clock counters drained at submit time. Stroke engine
    /// and node evaluators write to this directly via `record_*` helpers.
    /// Drained by `submit_final` — the engine merges the result into the
    /// stroke-level `StrokePerfStats`.
    pub perf: BrushPerfCounters,
}

impl<'a> BrushGpuContext<'a> {
    /// Submit the batched encoder and consume the context.
    ///
    /// All dab render passes in this batch are submitted in a single
    /// `queue.submit()` call — no per-dab submission needed thanks to
    /// dynamic uniform buffer offsets.
    ///
    /// Returns the per-context perf counters so the caller can fold them
    /// into the stroke-level `StrokePerfStats`. The final submit's wall
    /// clock is recorded into `submit_us` before returning.
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
            let us = t.elapsed().as_micros() as u64;
            self.perf.flush_submit_us = self.perf.flush_submit_us.saturating_add(us);
            self.perf.submits = self.perf.submits.saturating_add(1);
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
            let t = web_time::Instant::now();
            scratch.sync_read_mirror(
                self.device,
                &mut self.encoder,
                origin_x,
                origin_y,
                width,
                height,
            );
            let us = t.elapsed().as_micros() as u64;
            self.perf.read_mirror_copy_us = self.perf.read_mirror_copy_us.saturating_add(us);
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
