//! Stroke-recording file format. JSON, one brush stroke per file.
//!
//! Produced by `frontend/src/lib/strokeRecorder.ts` when the dev frontend
//! is loaded with `?_RECORD_STROKES=1`. Consumed by the
//! `stroke_replay_bench` binary to drive deterministic real-time replays
//! of real tablet input through a headless engine — primarily for
//! characterising stabilizer perf under workloads a synthetic diagonal
//! stroke can't reach.
//!
//! The on-disk struct is decoupled from `StrokeOp::BrushStroke`: the
//! engine's wire format is `Deserialize`-only and tagged with `op`; this
//! file format is a flat list of pen samples with no operation
//! discriminator. `RecordedEvent::to_stroke_op` bridges the two and
//! applies optional canvas-scale projection.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::engine::{DarklyEngine, StrokeOp};
use crate::layer::LayerId;

#[derive(Debug, Deserialize, Serialize)]
pub struct StrokeRecording {
    pub version: u32,
    #[serde(default)]
    pub recorded_at: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub events: Vec<RecordedEvent>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct RecordedEvent {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub x_tilt: f32,
    pub y_tilt: f32,
    pub rotation: f32,
    pub tangential_pressure: f32,
    /// Absolute `PointerEvent.timeStamp` at record time. Replay derives
    /// per-event offsets by subtracting `events[0].time_ms`; the value
    /// itself is also forwarded into the engine so the stabilizer sees
    /// the original cadence regardless of how the harness paces dispatches.
    pub time_ms: f64,
    pub cr: f32,
    pub cg: f32,
    pub cb: f32,
    pub ca: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error("io error reading recording: {0}")]
    Io(#[from] std::io::Error),
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported recording version: {0} (expected 1)")]
    UnsupportedVersion(u32),
    #[error("recording has no events")]
    Empty,
}

impl StrokeRecording {
    pub fn load(path: &Path) -> Result<Self, RecordingError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let recording: StrokeRecording = serde_json::from_reader(reader)?;
        recording.validate()?;
        Ok(recording)
    }

    pub fn from_json_str(s: &str) -> Result<Self, RecordingError> {
        let recording: StrokeRecording = serde_json::from_str(s)?;
        recording.validate()?;
        Ok(recording)
    }

    fn validate(&self) -> Result<(), RecordingError> {
        if self.version != 1 {
            return Err(RecordingError::UnsupportedVersion(self.version));
        }
        if self.events.is_empty() {
            return Err(RecordingError::Empty);
        }
        Ok(())
    }
}

impl RecordedEvent {
    /// Translate to a `StrokeOp::BrushStroke`, scaling `(x, y)` by `scale`.
    /// `scale = (1.0, 1.0)` is a no-op; other ratios let the harness
    /// replay a recording at a different canvas resolution than it was
    /// captured at.
    pub fn to_stroke_op(&self, scale: (f32, f32)) -> StrokeOp {
        StrokeOp::BrushStroke {
            x: self.x * scale.0,
            y: self.y * scale.1,
            pressure: self.pressure,
            x_tilt: self.x_tilt,
            y_tilt: self.y_tilt,
            rotation: self.rotation,
            tangential_pressure: self.tangential_pressure,
            time_ms: self.time_ms,
            cr: self.cr,
            cg: self.cg,
            cb: self.cb,
            ca: self.ca,
        }
    }
}

/// Per-event timing produced by [`replay`]. CPU wall-clock around a
/// single `engine.stroke_to` dispatch — the harness's perf signal.
#[derive(Debug, Clone, Default)]
pub struct EventTiming {
    pub index: usize,
    /// Milliseconds since `events[0].time_ms`. Useful for joining replay
    /// timings against the original recording timeline.
    pub t_offset_ms: f64,
    pub cpu_us: u64,
    /// Compute-pass shader time, summed across every paint-compute
    /// dispatch observed by the drain. Zero when the device doesn't
    /// expose `TIMESTAMP_QUERY`, or when the dispatch's readback callback
    /// hadn't fired yet at drain time (it rolls into the next event's
    /// reading; aggregate over the stroke is still correct).
    pub gpu_shader_ns: u64,
    /// GPU time for the pre-dispatch `copy_texture_to_buffer` that ingests
    /// the scratch texture into the compute buffer. Zero unless the device
    /// also exposes `TIMESTAMP_QUERY_INSIDE_ENCODERS`.
    pub gpu_sync_in_ns: u64,
    /// GPU time for the post-dispatch `copy_buffer_to_texture` that
    /// publishes the compute buffer back to the scratch texture. Same
    /// feature gating as `gpu_sync_in_ns`.
    pub gpu_sync_out_ns: u64,
    /// Number of paint-compute dispatches whose timestamps folded into
    /// the `gpu_*_ns` totals. Useful for divining the per-dispatch average
    /// when the engine emits more than one flush per event (e.g.,
    /// stabilizer rewinds).
    pub gpu_samples: u32,

    // --- Drained from `BrushPerfDelta` (Phase C). All us-scale CPU
    //     timings. Vectors are per-flush, in submission order. ---
    /// Host time inside `queue.submit()` calls during the event. Captures
    /// the "submit was back-pressured" tail that bloats `cpu_us` past
    /// shader time on saturated queues.
    pub submit_us: u64,
    /// Number of `queue.submit()` calls during the event.
    pub submits: u32,
    /// Number of paint-compute flushes (dispatches) during the event.
    pub compute_dispatches: u32,
    /// Dabs that flowed through the compute path during the event.
    pub dabs_total: u32,
    /// Sum of `union_w * union_h` across every flush of the event.
    pub union_bbox_area_total: u64,
    /// Per-flush dab counts (one entry per flush).
    pub compute_dabs_per_flush: Vec<u32>,
    /// Per-flush `union_w * union_h` in canvas pixels (parallel to
    /// `compute_dabs_per_flush`).
    pub compute_union_bbox_area_per_flush: Vec<u32>,
}

/// Pacing knob for [`replay`]. The engine's stabilizer reads `time_ms`
/// from each `StrokeOp` payload, not the wall-clock — both pacings drive
/// the engine identically. The choice only changes whether the harness
/// reproduces the original real-time experience or runs as fast as
/// possible (useful for tests and headless perf sweeps).
#[derive(Debug, Clone, Copy, Default)]
pub enum ReplayPacing {
    /// Sleep between events so wall-clock matches the recorded cadence.
    #[default]
    Realtime,
    /// Fire events back-to-back. Same engine behaviour, no sleeps.
    AsFastAsPossible,
}

/// Replay a recorded stroke through `engine` into `layer_id`. Recorded
/// `(x, y)` are scaled by `target_canvas / recording.canvas_*`, so a
/// recording captured at 4000×2000 played back at 1920×1080 fills the
/// same fraction of the new canvas. `time_ms` is forwarded verbatim, so
/// the stabilizer sees the original cadence regardless of `pacing`.
///
/// Returns one [`EventTiming`] per recorded event in order.
pub fn replay(
    engine: &mut DarklyEngine,
    recording: &StrokeRecording,
    layer_id: LayerId,
    target_canvas: (u32, u32),
    pacing: ReplayPacing,
) -> Vec<EventTiming> {
    let scale = (
        target_canvas.0 as f32 / recording.canvas_width as f32,
        target_canvas.1 as f32 / recording.canvas_height as f32,
    );
    let stream_start = recording.events[0].time_ms;
    let mut timings = Vec::with_capacity(recording.events.len());

    engine.begin_stroke(layer_id);

    let wall_start = Instant::now();
    for (i, ev) in recording.events.iter().enumerate() {
        if matches!(pacing, ReplayPacing::Realtime) {
            // Anchored at events[0] so the first dispatch fires immediately.
            let target_offset =
                Duration::from_secs_f64(((ev.time_ms - stream_start) / 1000.0).max(0.0));
            let now_offset = wall_start.elapsed();
            if target_offset > now_offset {
                std::thread::sleep(target_offset - now_offset);
            }
        }

        let op = ev.to_stroke_op(scale);
        let t = Instant::now();
        engine.stroke_to(op);
        let cpu_us = t.elapsed().as_micros() as u64;
        // Drain whatever paint-compute timestamps the GPU has finished
        // since the last event. Off-by-one delays roll over into the
        // next event's reading; the aggregate over the stroke stays correct.
        let ts = engine.drain_paint_compute_timestamps();
        let perf = engine.drain_brush_perf_delta();

        timings.push(EventTiming {
            index: i,
            t_offset_ms: ev.time_ms - stream_start,
            cpu_us,
            gpu_shader_ns: ts.shader_ns,
            gpu_sync_in_ns: ts.sync_in_ns,
            gpu_sync_out_ns: ts.sync_out_ns,
            gpu_samples: ts.sample_count,
            submit_us: perf.submit_us,
            submits: perf.submits,
            compute_dispatches: perf.compute_dispatches,
            dabs_total: perf.compute_dabs.min(u32::MAX as u64) as u32,
            union_bbox_area_total: perf.compute_union_bbox_area_total,
            compute_dabs_per_flush: perf.compute_dabs_per_flush,
            compute_union_bbox_area_per_flush: perf.compute_union_bbox_area_per_flush,
        });
    }

    engine.end_stroke();
    // Flush pending compositor work so a subsequent caller's first frame
    // doesn't pick up residue from this stroke.
    engine.render(0.0);

    // Any timestamps still in flight (the GPU hadn't finished the final
    // dispatch when the last event drained) fold into the last event's
    // gpu_* totals so the stroke aggregate stays consistent. The brush
    // perf delta likewise gets folded in for any flushes that landed
    // after the last per-event drain (e.g., end_stroke's final submit).
    if let Some(last) = timings.last_mut() {
        let tail_ts = engine.drain_paint_compute_timestamps();
        last.gpu_shader_ns = last.gpu_shader_ns.saturating_add(tail_ts.shader_ns);
        last.gpu_sync_in_ns = last.gpu_sync_in_ns.saturating_add(tail_ts.sync_in_ns);
        last.gpu_sync_out_ns = last.gpu_sync_out_ns.saturating_add(tail_ts.sync_out_ns);
        last.gpu_samples = last.gpu_samples.saturating_add(tail_ts.sample_count);

        let tail_perf = engine.drain_brush_perf_delta();
        last.submit_us = last.submit_us.saturating_add(tail_perf.submit_us);
        last.submits = last.submits.saturating_add(tail_perf.submits);
        last.compute_dispatches = last
            .compute_dispatches
            .saturating_add(tail_perf.compute_dispatches);
        last.dabs_total = last
            .dabs_total
            .saturating_add(tail_perf.compute_dabs.min(u32::MAX as u64) as u32);
        last.union_bbox_area_total = last
            .union_bbox_area_total
            .saturating_add(tail_perf.compute_union_bbox_area_total);
        last.compute_dabs_per_flush
            .extend(tail_perf.compute_dabs_per_flush);
        last.compute_union_bbox_area_per_flush
            .extend(tail_perf.compute_union_bbox_area_per_flush);
    }

    timings
}
