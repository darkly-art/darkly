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
    pub dab_flushes: u32,
    /// Total dabs that flowed through the compute path during the interval.
    pub flushed_dabs: u64,
    /// Sum of `union_w * union_h` across every flush during the interval.
    pub dab_union_bbox_area_total: u64,
    /// Per-flush dab counts for the flushes that landed during this
    /// interval, in the order they were submitted.
    pub dabs_per_flush: Vec<u32>,
    /// Per-flush `union_w * union_h` in canvas pixels, parallel to
    /// `dabs_per_flush`.
    pub dab_union_bbox_area_per_flush: Vec<u32>,
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
            dab_flushes: curr.dab_flushes.saturating_sub(prev.dab_flushes),
            flushed_dabs: (curr.flushed_dabs as u64).saturating_sub(prev.flushed_dabs as u64),
            dab_union_bbox_area_total: curr
                .dab_union_bbox_area
                .saturating_sub(prev.dab_union_bbox_area),
            dabs_per_flush: std::mem::take(&mut curr.dabs_per_flush),
            dab_union_bbox_area_per_flush: std::mem::take(&mut curr.dab_union_bbox_area_per_flush),
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

/// Dev instrumentation for input→frame latency and coalesced-sample fidelity.
///
/// Session-only, never serialized or undoable. Two signals, both surfaced on
/// [`crate::engine::EngineState`] for a dev HUD:
///
/// - **Worst-sample latency (`mean_latency_ms`):** present minus the *oldest*
///   input sample consumed that frame. With coalescing this reaches back a full
///   batch, so it reports the fidelity/quantization span, not what the user
///   perceives.
/// - **Tip latency (`tip_latency_ms`):** present minus the *newest* input
///   sample consumed that frame — the lag of the leading edge of the stroke,
///   the perceptually-relevant number and the one the Phase 2 (prediction) gate
///   keys off.
/// - **Fidelity (`samples_last_frame`):** how many stroke samples were consumed
///   on the most recent stroke frame. Rises above ~1 when coalesced pointer
///   events recover packets the browser would otherwise drop between frames.
///
/// Both latencies are EMAs over recent stroke frames (a stable HUD read rather
/// than a jittery per-frame value). Input timestamps and the frame timestamp
/// share the `performance.now()` timeline (the rAF `ts` the bridge hands
/// `render` as `time_secs`), so their difference is a real wall-clock latency.
/// Only the in-browser input→present segment is captured — OS→browser dispatch
/// has no timestamp before `e.timeStamp`, and display scan-out none after
/// present; both are the unmeasurable platform floor.
#[derive(Default)]
pub struct InputLatencyMeter {
    /// Oldest input `time_ms` consumed since the last readout; `None` when no
    /// stroke sample landed this frame.
    oldest_input_ms: Option<f64>,
    /// Newest input `time_ms` consumed since the last readout.
    newest_input_ms: Option<f64>,
    /// Stroke samples consumed since the last readout.
    samples_this_frame: u32,
    /// Samples consumed on the most recent frame that had any.
    samples_last_frame: u32,
    /// EMA of worst-sample latency (present − oldest) over recent stroke frames.
    mean_latency_ms: f32,
    /// EMA of tip latency (present − newest) over recent stroke frames.
    tip_latency_ms: f32,
}

impl InputLatencyMeter {
    /// Record one consumed input sample carrying `time_ms` (performance.now
    /// timeline). Called for every stroke sample as it is applied, so a
    /// coalesced burst's oldest wins the worst-sample latency and its newest
    /// wins the tip latency.
    pub fn record_sample(&mut self, time_ms: f64) {
        self.oldest_input_ms = Some(match self.oldest_input_ms {
            Some(o) => o.min(time_ms),
            None => time_ms,
        });
        self.newest_input_ms = Some(match self.newest_input_ms {
            Some(n) => n.max(time_ms),
            None => time_ms,
        });
        self.samples_this_frame += 1;
    }

    /// Fold this frame's samples into the rolling stats and reset the per-frame
    /// accumulators. `frame_time_ms` is the rAF present timestamp on the same
    /// timeline as the recorded samples. Frames with no stroke sample don't
    /// perturb the latency stats (they would read as a spurious near-zero).
    pub fn readout(&mut self, frame_time_ms: f64) {
        if let (Some(oldest), Some(newest)) =
            (self.oldest_input_ms.take(), self.newest_input_ms.take())
        {
            self.mean_latency_ms = ema(
                self.mean_latency_ms,
                input_latency_ms(frame_time_ms, oldest),
            );
            self.tip_latency_ms = ema(self.tip_latency_ms, input_latency_ms(frame_time_ms, newest));
            self.samples_last_frame = self.samples_this_frame;
        }
        self.samples_this_frame = 0;
    }

    /// EMA of worst-sample input→frame latency (ms) over recent stroke frames.
    pub fn mean_latency_ms(&self) -> f32 {
        self.mean_latency_ms
    }

    /// EMA of tip (newest-sample) input→frame latency (ms) over recent stroke
    /// frames — the perceptual lag and the Phase 2 gate metric.
    pub fn tip_latency_ms(&self) -> f32 {
        self.tip_latency_ms
    }

    /// Stroke samples consumed on the most recent stroke frame.
    pub fn samples_last_frame(&self) -> u32 {
        self.samples_last_frame
    }
}

/// Exponential moving average, seeded on the first reading so it converges fast
/// rather than crawling up from zero.
fn ema(prev: f32, sample: f32) -> f32 {
    if prev == 0.0 {
        sample
    } else {
        prev * 0.9 + sample * 0.1
    }
}

/// Input-to-frame latency in milliseconds: the frame present timestamp minus an
/// input sample's timestamp, clamped at zero. Both operands live on the
/// `performance.now()` timeline. Pure so it can be unit-tested without a GPU
/// surface.
pub fn input_latency_ms(frame_time_ms: f64, input_ms: f64) -> f32 {
    (frame_time_ms - input_ms).max(0.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_is_present_minus_oldest_sample() {
        // The reported latency is present-time minus the oldest input sample.
        assert_eq!(input_latency_ms(1050.0, 1032.0), 18.0);
        // A frame with no earlier sample (frame == sample) reads zero.
        assert_eq!(input_latency_ms(1000.0, 1000.0), 0.0);
        // Clamped at zero — a sample timestamped after the frame (clock skew /
        // reordering) never reports negative latency.
        assert_eq!(input_latency_ms(1000.0, 1005.0), 0.0);
    }

    #[test]
    fn meter_splits_worst_sample_from_tip_over_a_coalesced_burst() {
        let mut m = InputLatencyMeter::default();
        // A coalesced burst: three samples land this frame, out of order. The
        // oldest drives worst-sample latency; the newest drives tip latency.
        m.record_sample(1040.0);
        m.record_sample(1020.0);
        m.record_sample(1030.0);
        m.readout(1050.0);
        assert_eq!(m.mean_latency_ms(), 30.0); // 1050 - 1020 (oldest), seeded
        assert_eq!(m.tip_latency_ms(), 10.0); // 1050 - 1040 (newest), seeded
        assert_eq!(m.samples_last_frame(), 3); // fidelity counter (M2)
    }

    #[test]
    fn meter_ignores_frames_without_samples() {
        let mut m = InputLatencyMeter::default();
        m.record_sample(1000.0);
        m.readout(1010.0);
        let seeded = m.mean_latency_ms();
        assert_eq!(seeded, 10.0);
        // A frame with no stroke sample must not perturb the stats (it would
        // otherwise read as a spurious near-zero latency).
        m.readout(9999.0);
        assert_eq!(m.mean_latency_ms(), seeded);
        assert_eq!(m.samples_last_frame(), 1); // unchanged from the last stroke frame
    }
}
