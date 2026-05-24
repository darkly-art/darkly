//! Brush perf instrumentation — bench-facing extraction + frame phases.
//!
//! [`BrushPerfCounters`] itself lives on [`crate::brush::gpu_context`]
//! because it's a field on `BrushGpuContext`. The engine accumulates
//! contexts' counters into its own `brush_perf` field via `+=`. The bench
//! harness drains an interval-delta via [`BrushPerfDelta::between`].
//!
//! ## Where to add new measurements
//!
//! New *fine-grained* timing should grow the GPU-timestamp slot pattern in
//! `PaintComputeTimestamps` (6 slots today, easily grown), **not** new CPU
//! `record_*` methods on `BrushPerfCounters`. `Instant::now()` brackets in
//! the brush hot path are non-zero overhead in production and sprawl
//! during investigations — the previous `[stab-perf]` log carried ~25
//! sub-buckets that all paid that cost. Keep `BrushPerfCounters` small
//! and stable; reach for timestamps when you need finer attribution.

use crate::brush::gpu_context::BrushPerfCounters;

/// Per-interval brush perf delta returned by
/// [`crate::engine::DarklyEngine::drain_brush_perf_delta`]. Scalars are
/// differences against the previous snapshot; vectors are taken
/// whole-cloth from the current counter (and reset to empty there).
///
/// Bench-only — the WASM bridge never calls the drain.
#[derive(Default, Debug, Clone)]
pub struct BrushPerfDelta {
    /// Wall-clock microseconds spent inside `queue.submit()` (final +
    /// flush) during this interval.
    pub submit_us: u64,
    /// Number of `queue.submit()` calls issued during this interval.
    pub submits: u32,
    /// Number of paint-compute flushes that landed during this interval.
    pub compute_dispatches: u32,
    /// Total dabs that flowed through the compute path during the interval.
    pub compute_dabs: u64,
    /// Sum of `union_w * union_h` across every flush during the interval.
    pub compute_union_bbox_area_total: u64,
    /// Per-flush dab counts for the flushes that landed during this
    /// interval, in the order they were submitted.
    pub compute_dabs_per_flush: Vec<u32>,
    /// Per-flush `union_w * union_h` in canvas pixels, parallel to
    /// `compute_dabs_per_flush`.
    pub compute_union_bbox_area_per_flush: Vec<u32>,
}

impl BrushPerfDelta {
    /// Difference between two counter snapshots. Scalars are
    /// `saturating_sub`'d; the per-flush vectors are taken from `curr`
    /// via `mem::take` (so `curr`'s vectors are empty afterwards — the
    /// engine resnapshots `prev` from `curr` after this call, which is
    /// why that's correct).
    pub(crate) fn between(curr: &mut BrushPerfCounters, prev: &BrushPerfCounters) -> Self {
        Self {
            submit_us: curr.submit_us.saturating_sub(prev.submit_us),
            submits: curr.submits.saturating_sub(prev.submits),
            compute_dispatches: curr
                .compute_dispatches
                .saturating_sub(prev.compute_dispatches),
            compute_dabs: (curr.compute_dabs as u64).saturating_sub(prev.compute_dabs as u64),
            compute_union_bbox_area_total: curr
                .compute_union_bbox_area
                .saturating_sub(prev.compute_union_bbox_area),
            compute_dabs_per_flush: std::mem::take(&mut curr.compute_dabs_per_flush),
            compute_union_bbox_area_per_flush: std::mem::take(
                &mut curr.compute_union_bbox_area_per_flush,
            ),
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
