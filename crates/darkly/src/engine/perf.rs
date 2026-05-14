//! Stroke + frame perf instrumentation.
//!
//! Wall-clock timing only — every duration in this module is CPU time
//! (`web_time::Instant::elapsed`), which on WASM resolves to
//! `performance.now()` deltas.  None of these counters reflect GPU shader
//! cost; they measure encoder building, IPC, and host-side bookkeeping.
//! GPU shader timing would require `TIMESTAMP_QUERY` and async resolution
//! — not done here.
//!
//! Why CPU timing is useful anyway: the "drain >99% of frame time" finding
//! says the lag lives somewhere on the host side.  Splitting host time per
//! phase / per dab tells us which host-side operations dominate.

use crate::brush::gpu_context::BrushPerfCounters;

/// Per-stroke perf accumulator for diagnosing stabilization lag.
///
/// Reset at `begin_stroke`, accumulated by every `brush_stroke_to`, drained
/// (and emitted as a `log::info!` summary) at `end_stroke`.
///
/// Fields are grouped:
/// - event-level counters (event count, divergence count, etc.)
/// - phase timing (cumulative microseconds across the stroke)
/// - per-dab timing (drained from `BrushPerfCounters` at each submit)
/// - submit count
///
/// All times are microseconds, all counts are u32 (saturating).  The
/// summary log divides by `events` to get per-event averages.
#[derive(Default)]
pub(crate) struct StrokePerfStats {
    // --- Event-level counters ---
    pub events: u32,
    pub divergence_events: u32,
    pub full_rerender_events: u32,
    pub total_rerender_range: u64,
    pub max_rerender_range: u32,
    pub total_elapsed_us: u64,
    pub max_event_us: u64,
    pub last_max_div_window: usize,
    pub last_spacing: usize,
    /// Largest BrushStroke backlog the WASM bridge observed in a single
    /// drain. Fed by `record_input_backlog`. `0` if the bridge never fed
    /// (e.g. a native test). High values indicate render-lag-induced input
    /// queueing.
    pub max_queue_backlog: u32,

    // --- Phase timing (cumulative across stroke, microseconds) ---
    pub total_phase_stabilize_us: u64,
    pub total_phase_rewind_us: u64,
    pub total_phase_restore_us: u64,
    pub total_phase_segments_us: u64,
    pub total_phase_tail_us: u64,
    pub total_phase_commit_us: u64,
    /// Total wall-clock time spent inside `queue.submit()` calls during
    /// the stroke. The IPC + driver thunk cost of each submit; on
    /// WebGPU/Chromium each submit pays a per-call cost regardless of
    /// command volume.
    pub total_submit_us: u64,
    pub total_submits: u32,

    // --- Per-dab timing (drained from BrushPerfCounters at each submit) ---
    pub total_dabs: u64,
    pub max_dabs_per_event: u32,
    /// Sum of host-side time per `place_dab` invocation. Includes graph
    /// eval + every GPU node's encoder ops + pool acquire + flush check.
    pub total_dab_us: u64,
    /// Sum of CPU node-graph evaluation time (`execute_cpu` +
    /// `execute_gpu` invocation overhead).
    pub total_dab_graph_eval_us: u64,
    /// Sum of host time recording stamp render passes
    /// (`encode_stamp_pass`).
    pub total_dab_stamp_us: u64,
    /// Sum of host time recording color_output composite render passes
    /// (the `begin_render_pass` + draw + drop sequence in
    /// `color_output::evaluate_gpu`).
    pub total_dab_composite_us: u64,
    /// Sum of host time spent issuing `copy_texture_to_texture` for the
    /// read mirror (`Scratch::sync_read_mirror`).
    pub total_dab_read_mirror_us: u64,
    /// Sum of host time spent in `DabTexturePool::acquire_sized`.
    pub total_dab_pool_acquire_us: u64,
    /// Sum of host time spent in `BrushGpuContext::flush_if_needed` when
    /// it actually flushes (submits + resets rings). Excludes the
    /// no-op-fast-path cost.
    pub total_dab_flush_us: u64,

    // --- Inner-loop buckets (added to find the per-dab unattributed gap) ---
    /// Sum of host time inside `runner.execute_gpu(gpu)`. Includes
    /// every GPU-node evaluator; the per-node buckets
    /// (`total_dab_stamp_us`, `total_dab_composite_us`,
    /// `total_dab_read_mirror_us`) are subsets.
    pub total_dab_execute_gpu_us: u64,
    /// Sum of host time spent in `DabTexturePool::release_all`.
    pub total_dab_release_all_us: u64,
    /// Sum of host time spent in `place_dab`'s post-`execute_gpu`
    /// bookkeeping (terminal-output reads, canvas-bbox math,
    /// save-points push).
    pub total_dab_post_us: u64,

    /// Total GPU steps the brush-graph runner iterated over across the
    /// whole stroke. Divide by `total_dabs` for steps-per-dab.
    pub total_gpu_steps: u64,
    /// Sum of host time inside `gather_inputs` across all GPU steps.
    pub total_gather_inputs_us: u64,
    /// Sum of host time inside per-step output write-back loops.
    pub total_step_outputs_us: u64,
    /// Sum of host time inside `evaluator.evaluate_gpu(...)` bodies. The
    /// node-pass sub-buckets (`total_dab_stamp_us`,
    /// `total_dab_composite_us`, etc.) are subsets.
    pub total_evaluate_gpu_call_us: u64,
    /// Sum of host time inside promoted-CPU `evaluate_cpu(...)` calls
    /// made from `dispatch_gpu`.
    pub total_evaluate_cpu_in_gpu_us: u64,

    /// Sum of host time inside `prepare_dab_canvas_copy` (includes the
    /// `read_mirror_copy` cost as a subset).
    pub total_prepare_canvas_copy_us: u64,
    /// Sum of host time inside `write_composite_uniforms`.
    pub total_write_composite_uniforms_us: u64,
    /// Sum of host time inside `write_stamp_uniforms` (subset of
    /// `total_dab_stamp_us`).
    pub total_write_stamp_uniforms_us: u64,
    /// Sum of host time inside `ctx.input(...)` lookups at the top of
    /// `color_output::evaluate_gpu`.
    pub total_ctx_input_us: u64,
}

impl StrokePerfStats {
    /// Merge per-event counters drained from a `BrushGpuContext` at submit
    /// time. Counters are cumulative across all contexts created during a
    /// single `brush_stroke_to`; the caller decides when to track the
    /// per-event peak (see `update_max_dabs_per_event`).
    pub fn merge_brush(&mut self, c: BrushPerfCounters) {
        self.total_dabs = self.total_dabs.saturating_add(c.dabs_placed as u64);
        self.total_dab_us = self.total_dab_us.saturating_add(c.dab_total_us);
        self.total_dab_graph_eval_us = self.total_dab_graph_eval_us.saturating_add(c.graph_eval_us);
        self.total_dab_stamp_us = self.total_dab_stamp_us.saturating_add(c.stamp_pass_us);
        self.total_dab_composite_us = self
            .total_dab_composite_us
            .saturating_add(c.composite_pass_us);
        self.total_dab_read_mirror_us = self
            .total_dab_read_mirror_us
            .saturating_add(c.read_mirror_copy_us);
        self.total_dab_pool_acquire_us = self
            .total_dab_pool_acquire_us
            .saturating_add(c.pool_acquire_us);
        self.total_dab_flush_us = self.total_dab_flush_us.saturating_add(c.flush_submit_us);
        self.total_dab_execute_gpu_us = self
            .total_dab_execute_gpu_us
            .saturating_add(c.execute_gpu_us);
        self.total_dab_release_all_us = self
            .total_dab_release_all_us
            .saturating_add(c.release_all_us);
        self.total_dab_post_us = self.total_dab_post_us.saturating_add(c.post_dab_us);
        self.total_gpu_steps = self.total_gpu_steps.saturating_add(c.gpu_steps);
        self.total_gather_inputs_us = self
            .total_gather_inputs_us
            .saturating_add(c.gather_inputs_us);
        self.total_step_outputs_us = self
            .total_step_outputs_us
            .saturating_add(c.step_outputs_us);
        self.total_evaluate_gpu_call_us = self
            .total_evaluate_gpu_call_us
            .saturating_add(c.evaluate_gpu_call_us);
        self.total_evaluate_cpu_in_gpu_us = self
            .total_evaluate_cpu_in_gpu_us
            .saturating_add(c.evaluate_cpu_in_gpu_us);
        self.total_prepare_canvas_copy_us = self
            .total_prepare_canvas_copy_us
            .saturating_add(c.prepare_canvas_copy_us);
        self.total_write_composite_uniforms_us = self
            .total_write_composite_uniforms_us
            .saturating_add(c.write_composite_uniforms_us);
        self.total_write_stamp_uniforms_us = self
            .total_write_stamp_uniforms_us
            .saturating_add(c.write_stamp_uniforms_us);
        self.total_ctx_input_us = self.total_ctx_input_us.saturating_add(c.ctx_input_us);
        self.total_submit_us = self.total_submit_us.saturating_add(c.submit_us);
        self.total_submits = self.total_submits.saturating_add(c.submits);
    }

    /// Track the largest dab count seen in a single `brush_stroke_to`.
    /// Call after summing all `BrushPerfCounters` for one event.
    pub fn update_max_dabs_per_event(&mut self, dabs_this_event: u32) {
        if dabs_this_event > self.max_dabs_per_event {
            self.max_dabs_per_event = dabs_this_event;
        }
    }
}

/// Most recent `engine.render()` sub-phase timings, in microseconds.
/// Overwritten each frame. Read by the WASM bridge's slow-frame log so the
/// breakdown is surfaced alongside the bridge-side drain/render timing
/// without having to plumb a return value out of `render`.
#[derive(Default, Clone, Copy)]
pub struct FrameRenderPhases {
    pub poll_us: u64,
    pub thumb_us: u64,
    pub anim_us: u64,
    pub compositor_us: u64,
}
