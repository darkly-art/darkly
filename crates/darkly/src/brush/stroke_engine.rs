//! Stroke engine — bridges pen input events to the brush node graph.
//!
//! Owns the `BrushGraphRunner` for the stroke duration and handles:
//! - Storing raw events in `StrokeRecord` (for re-rendering)
//! - Stabilization (retroactive stroke reshaping via pluggable algorithm)
//! - Computing derived sensor values (speed, distance, angle, tilt)
//! - Interpolating between events and placing dabs at spacing intervals
//! - Evaluating the brush graph per dab (CPU + GPU)
//! - Per-dab save points for rewind capability

use super::dab_pool::DAB_REFERENCE_SIZE;
use super::eval::BrushGraphRunner;
use super::gpu_context::BrushGpuContext;
use super::interpolation::{lerp_paint_info, CatmullRomSegment};
use super::paint_info::{PaintInformation, StrokeRecord};
use super::save_points::SavePointStore;
use super::spacing::SpacingConfig;
use super::stabilizer::{StabilizeResult, StabilizerAlgorithm};

/// Snapshot of the stroke engine's render state at a specific dab.
///
/// Used by the checkpoint system to restore the engine to a known state
/// and re-render only from that point forward, instead of from scratch.
#[derive(Clone)]
pub struct RenderCheckpoint {
    pub last_point: Option<PaintInformation>,
    pub accumulated_distance: f32,
    pub leftover_distance: f32,
    pub last_dab_size: [f32; 2],
    pub last_dab_pos: Option<[f32; 2]>,
    pub dab_count: u32,
}

/// Reference fade distance in pixels.  The fade sensor goes from 0 to 1
/// over this distance, then clamps at 1.  Configurable per-brush later.
const FADE_DISTANCE_PX: f32 = 1000.0;

/// Drives a single brush stroke from begin to end.
///
/// Created by the engine at stroke start, fed pointer events via `move_to`,
/// and consumed at stroke end to yield a `StrokeRecord`.
pub struct StrokeEngine {
    runner: BrushGraphRunner,
    record: StrokeRecord,
    spacing: SpacingConfig,

    /// Pluggable stabilizer algorithm (pass-through when no stabilization).
    stabilizer: Box<dyn StabilizerAlgorithm>,

    /// Per-dab save points for rewind capability.
    pub save_points: SavePointStore,

    /// Last processed point for interpolation (post-derived-values).
    last_point: Option<PaintInformation>,
    /// Cumulative distance along the stroke path (in pixels).
    accumulated_distance: f32,
    /// Distance remaining from the last segment that didn't reach the next
    /// spacing threshold — carried forward to the next segment.
    leftover_distance: f32,
    /// Dab size [w, h] from the last evaluated dab (for spacing).
    last_dab_size: [f32; 2],
    /// Position of the most recently *emitted* dab — source-of-truth for
    /// `PaintInformation.motion` (per-dab delta, populated in `place_dab`).
    /// Distinct from `last_point` which tracks the previous stabilized
    /// *event*. Reset to `None` at stroke start and on full re-render.
    last_dab_pos: Option<[f32; 2]>,
    /// Running dab index within the stroke.
    dab_count: u32,

    /// Stroke seed for deterministic per-dab randomness.  Passed to
    /// the runner so random nodes can generate independent sequences.
    stroke_seed: u32,
}

impl StrokeEngine {
    /// Create a new stroke engine.
    ///
    /// `runner` is a pre-compiled brush graph.  `color` is the foreground
    /// color (linear RGBA).  `spacing` controls dab placement.
    /// `stabilizer` is the stroke stabilization algorithm.
    pub fn new(
        runner: BrushGraphRunner,
        color: [f32; 4],
        spacing: SpacingConfig,
        stabilizer: Box<dyn StabilizerAlgorithm>,
    ) -> Self {
        let stroke_seed = web_time::SystemTime::now()
            .duration_since(web_time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u32)
            .unwrap_or(42);

        let d = Self::default_diameter();
        Self {
            runner,
            record: StrokeRecord::new(color, "default".into()),
            spacing,
            stabilizer,
            save_points: SavePointStore::new(),
            last_point: None,
            accumulated_distance: 0.0,
            leftover_distance: 0.0,
            last_dab_size: [d, d],
            last_dab_pos: None,
            dab_count: 0,
            stroke_seed,
        }
    }

    /// Default dab diameter for initial spacing (before the first dab is evaluated).
    fn default_diameter() -> f32 {
        DAB_REFERENCE_SIZE as f32 * 0.5
    }

    /// The effective canvas-space diameter for spacing and bounding rect.
    fn effective_diameter(&self) -> f32 {
        self.last_dab_size[0].max(self.last_dab_size[1])
    }

    /// Feed a raw pointer event to the stabilizer.
    ///
    /// Returns the stabilization result (divergence info).  The caller
    /// is responsible for rewind + re-render when divergence occurs.
    pub fn stabilize(&mut self, raw: PaintInformation) -> StabilizeResult {
        self.record.push(raw);
        self.stabilizer.push(raw)
    }

    /// The stabilizer's conservative max divergence window (vector indices).
    pub fn max_divergence_window(&self) -> usize {
        self.stabilizer.max_divergence_window()
    }

    /// Number of points in the stabilized polyline.
    pub fn stabilizer_len(&self) -> usize {
        self.stabilizer.len()
    }

    /// Capture the current render state as a checkpoint.
    pub fn capture_render_state(&self) -> RenderCheckpoint {
        RenderCheckpoint {
            last_point: self.last_point,
            accumulated_distance: self.accumulated_distance,
            leftover_distance: self.leftover_distance,
            last_dab_size: self.last_dab_size,
            last_dab_pos: self.last_dab_pos,
            dab_count: self.dab_count,
        }
    }

    /// Restore render state from a checkpoint.
    pub fn restore_render_state(&mut self, checkpoint: &RenderCheckpoint) {
        self.last_point = checkpoint.last_point;
        self.accumulated_distance = checkpoint.accumulated_distance;
        self.leftover_distance = checkpoint.leftover_distance;
        self.last_dab_size = checkpoint.last_dab_size;
        self.last_dab_pos = checkpoint.last_dab_pos;
        self.dab_count = checkpoint.dab_count;
    }

    /// Reset rendering state for a full re-render from scratch.
    ///
    /// Call this before `render_from_stabilized()` when the stabilizer
    /// reports divergence and the stroke buffer has been rewound.
    pub fn reset_render_state(&mut self) {
        self.last_point = None;
        self.accumulated_distance = 0.0;
        self.leftover_distance = 0.0;
        let d = Self::default_diameter();
        self.last_dab_size = [d, d];
        self.last_dab_pos = None;
        self.dab_count = 0;
        self.save_points.clear();
    }

    /// Compute the per-dab motion vector for a dab about to be placed at
    /// `pos`, and advance the last-dab-position tracker. Thin wrapper over
    /// the free function so the motion contract can be unit-tested without
    /// constructing a full `StrokeEngine` (which would require a runner +
    /// stabilizer + GPU).
    fn next_dab_motion(&mut self, pos: [f32; 2]) -> [f32; 2] {
        advance_dab_motion(&mut self.last_dab_pos, pos)
    }

    /// Render dabs along the stabilized polyline starting from `start_vector_index`.
    ///
    /// Used for partial re-render after checkpoint restoration. Walks the
    /// stabilized polyline from `start_vector_index` to tip, computing derived
    /// values (speed, distance, angle) between consecutive points, and
    /// placing dabs at spacing intervals.
    pub fn render_from_stabilized_range(
        &mut self,
        gpu: &mut BrushGpuContext,
        start_vector_index: usize,
    ) {
        let end = self.stabilizer.len().saturating_sub(1);
        self.render_from_stabilized_range_to(gpu, start_vector_index, end);
    }

    /// Render dabs along the stabilized polyline from `start_vector_index`
    /// to `end_vector_index` (inclusive).
    ///
    /// Used for segmented rendering with checkpoints between segments.
    /// The engine's render state is left ready to continue from end+1.
    pub fn render_from_stabilized_range_to(
        &mut self,
        gpu: &mut BrushGpuContext,
        start_vector_index: usize,
        end_vector_index: usize,
    ) {
        // Copy the stabilized polyline to avoid borrow conflict with &mut self.
        let stabilized: Vec<PaintInformation> = self.stabilizer.stabilized().to_vec();
        if stabilized.is_empty() {
            return;
        }

        let start = start_vector_index.min(stabilized.len());
        let end = end_vector_index.min(stabilized.len() - 1);

        // When resuming from a checkpoint, snap last_point.pos to the current
        // stabilized position.  Between checkpoint capture and now, intermediate
        // frames may have shifted the polyline — the checkpoint's last_point
        // reflects the old position.  Without this, the first segment bridges
        // from the old position to the new next point, creating a tangent
        // discontinuity ("broken chain" artifact at corners).
        if start > 0 {
            if let Some(ref mut lp) = self.last_point {
                if let Some(current) = stabilized.get(start - 1) {
                    lp.pos = current.pos;
                }
            }
        }

        // Walk the polyline, computing derived values and placing dabs.
        for i in start..=end {
            let raw = stabilized[i];
            let mut info = raw;

            // First point of the stroke: no segment to place dabs along.
            if self.last_point.is_none() {
                info.derive_sensors(None, 0.0);
                self.place_dab(&info, gpu, i);
                self.last_point = Some(info);
                self.save_points
                    .finalize_render_state(i, self.capture_render_state());
                continue;
            }

            let prev = self.last_point.unwrap();

            // Build Catmull-Rom segment between prev (p1) and info (p2).
            // Outer control points use stabilized neighbours when available;
            // degenerate fallback duplicates the endpoint at stroke edges.
            let p0_pt = if i >= 2 { stabilized[i - 2] } else { prev };
            let p1_pt = prev;
            let p2_pt = info;
            let p3_pt = if i + 1 < stabilized.len() {
                stabilized[i + 1]
            } else {
                info
            };

            let seg = CatmullRomSegment::new(&p0_pt, &p1_pt, &p2_pt, &p3_pt);
            let arc_len = seg.arc_length();

            // Segment-derived sensors use the Catmull-Rom arc length —
            // chord distance would under-count on curved strokes.
            info.derive_sensors(Some(&prev), arc_len);
            self.accumulated_distance = info.distance;

            if arc_len < 0.001 {
                self.last_point = Some(info);
                self.save_points
                    .finalize_render_state(i, self.capture_render_state());
                continue;
            }

            let mut traveled = self.leftover_distance;
            while traveled < arc_len {
                // Position comes from the curve; sensors lerp between
                // endpoints so they can't overshoot (pressure stays in-range,
                // time stays monotonic, etc.).
                let cr_dab = seg.eval_at_distance(traveled);
                let t_lerp = traveled / arc_len;
                let mut dab_info = lerp_paint_info(&prev, &info, t_lerp);
                dab_info.pos = cr_dab.pos;
                self.place_dab(&dab_info, gpu, i);
                let step = self.spacing.distance(self.effective_diameter());
                debug_assert!(
                    step >= super::spacing::ABSOLUTE_MIN_SPACING_PX,
                    "dab spacing dropped below 1px: {step}"
                );
                traveled += step;
            }

            self.leftover_distance = traveled - arc_len;
            self.last_point = Some(info);

            // Capture end-of-segment state on ALL save points for this vector
            // index.  This represents "everything through vector index i is
            // fully processed" — the checkpoint restore starts from i+1.
            self.save_points
                .finalize_render_state(i, self.capture_render_state());
        }

        // Phase-end flush for dab-batching terminals (paint, watercolor_batched):
        // dispatch the batched dab queue before this phase's submit_final.
        // Fragment-path terminals no-op here.
        self.runner.flush_dabs(gpu);
    }

    /// Process a raw pointer event — stabilize and render in one step.
    ///
    /// Convenience method that combines `stabilize()` + `render_from_stabilized_tail()`.
    /// Used by the fallback path when no stroke buffer is active.
    /// When divergence occurs, the caller must handle rewind externally.
    pub fn move_to(&mut self, raw: PaintInformation, gpu: &mut BrushGpuContext) -> StabilizeResult {
        let result = self.stabilize(raw);
        if result.divergence_index.is_none() {
            self.render_from_stabilized_tail(gpu);
        }
        result
    }

    /// Evaluate the brush graph for a single dab at the given position.
    fn place_dab(
        &mut self,
        info: &PaintInformation,
        gpu: &mut BrushGpuContext,
        vector_index: usize,
    ) {
        let mut dab_info = *info;
        dab_info.fade = (dab_info.distance / FADE_DISTANCE_PX).min(1.0);
        // Motion is a per-dab quantity — the previous-dab → this-dab delta.
        // Interpolators leave it zero (they have no view of dab order); we
        // fill it here so smudge sees the correct smear-sample offset.
        dab_info.motion = self.next_dab_motion(dab_info.pos);

        self.runner.clear_slots();
        self.runner.seed_sensors(
            &dab_info,
            self.record.color,
            self.stroke_seed,
            self.dab_count,
        );
        self.runner.execute_cpu();

        // Per-dab context state: reset the read-mirror cache so the first
        // node that needs a canvas region this dab actually issues the copy.
        gpu.reset_per_dab_read_cache();
        // Reset the write-bbox accumulator so each terminal's passes can
        // publish their footprint fresh. Read back after execute_gpu below.
        gpu.dab_write_canvas_bbox = None;
        self.runner.execute_gpu(gpu);

        gpu.dab_pool.release_all();

        gpu.flush_if_needed();

        // Update dab size from dab source node output (procedural, stamp,
        // or warp terminals like liquify that report an effective radius).
        for node_type in &[
            "procedural",
            "stamp",
            "liquify",
            "paint",
            "watercolor_batched",
        ] {
            if let Some(slot) = self.runner.find_output_slot(node_type, "dab_size") {
                if let Some(val) = self.runner.read_slot(slot) {
                    let size = val.as_vec2();
                    if size[0] > 0.0 && size[1] > 0.0 {
                        self.last_dab_size = size;
                        break;
                    }
                }
            }
        }

        // Dab bounding box for save points, in canvas coords. Prefer the
        // footprint the terminal actually wrote (post-scatter, post-anything
        // else the graph did). Fall back to the `info.pos ± radius`
        // envelope for graphs without a scratch-writing terminal, so they
        // still get sensible checkpoint bounds.
        let canvas_bbox = gpu.dab_write_canvas_bbox.unwrap_or_else(|| {
            let diameter = self.effective_diameter();
            let half = diameter * 0.5;
            let x = (info.pos[0] - half).floor() as i32;
            let y = (info.pos[1] - half).floor() as i32;
            let x2 = (info.pos[0] + half).ceil() as i32;
            let y2 = (info.pos[1] + half).ceil() as i32;
            crate::coord::CanvasRect::from_xywh(
                x,
                y,
                (x2 - x).max(0) as u32,
                (y2 - y).max(0) as u32,
            )
        });
        // Render state is captured at end-of-segment, not per-dab.
        // Push a placeholder; the loop in render_from_stabilized_range
        // overwrites the last save point's render_state after each segment.
        self.save_points.push(
            canvas_bbox,
            vector_index,
            RenderCheckpoint {
                last_point: None,
                accumulated_distance: 0.0,
                leftover_distance: 0.0,
                last_dab_size: [0.0, 0.0],
                last_dab_pos: None,
                dab_count: 0,
            },
        );

        self.dab_count += 1;
        gpu.perf.record_dab();
    }

    /// Render only the tail of the stabilized polyline — the latest point.
    ///
    /// Used when the stabilizer reports no divergence (only new points added).
    /// The engine's internal state (last_point, leftover_distance) is still
    /// valid from the previous render, so we continue from where we left off.
    pub fn render_from_stabilized_tail(&mut self, gpu: &mut BrushGpuContext) {
        let stabilized = self.stabilizer.stabilized();
        let len = stabilized.len();
        if len == 0 {
            return;
        }

        let raw_pt = stabilized[len - 1];
        let mut info = raw_pt;

        if self.last_point.is_none() {
            info.derive_sensors(None, 0.0);
            self.place_dab(&info, gpu, len - 1);
            self.last_point = Some(info);
            self.save_points
                .finalize_render_state(len - 1, self.capture_render_state());
            return;
        }

        let prev = self.last_point.unwrap();

        // Tip segment: no future sample yet, so p3 = p2 (degenerate).
        // The next input event re-renders this segment with proper
        // lookahead via the synthesized tip-correction divergence.
        let p0_pt = if len >= 3 { stabilized[len - 3] } else { prev };
        let p1_pt = prev;
        let p2_pt = info;
        let p3_pt = info;

        let seg = CatmullRomSegment::new(&p0_pt, &p1_pt, &p2_pt, &p3_pt);
        let arc_len = seg.arc_length();

        info.derive_sensors(Some(&prev), arc_len);
        self.accumulated_distance = info.distance;

        if arc_len < 0.001 {
            self.last_point = Some(info);
            return;
        }

        let mut traveled = self.leftover_distance;
        while traveled < arc_len {
            let cr_dab = seg.eval_at_distance(traveled);
            let t_lerp = traveled / arc_len;
            let mut dab_info = lerp_paint_info(&prev, &info, t_lerp);
            dab_info.pos = cr_dab.pos;
            self.place_dab(&dab_info, gpu, len - 1);
            let step = self.spacing.distance(self.effective_diameter());
            debug_assert!(
                step >= super::spacing::ABSOLUTE_MIN_SPACING_PX,
                "dab spacing dropped below 1px: {step}"
            );
            traveled += step;
        }

        self.leftover_distance = traveled - arc_len;
        self.last_point = Some(info);
        self.save_points
            .finalize_render_state(len - 1, self.capture_render_state());

        // Phase-end flush for compute-path terminals. See sibling call
        // in `render_from_stabilized_range_to`.
        self.runner.flush_dabs(gpu);
    }

    /// Delegate the stroke-start / rewind-boundary lifecycle hook to every
    /// GPU terminal in the graph. Called by the engine at the start of a
    /// stroke and at every rewind boundary (full or partial) — the paint
    /// terminal clears its scratch here; other terminals (warp, smudge, …)
    /// may copy the pre-stroke layer, etc.
    pub fn begin_stroke(&mut self, gpu: &mut BrushGpuContext) {
        self.runner.begin_stroke(gpu);
    }

    /// Delegate the per-pen-event commit hook to every GPU terminal. Called
    /// once per pen event after the event's dabs have rendered into the
    /// scratch.
    pub fn commit(&mut self, gpu: &mut BrushGpuContext) {
        self.runner.commit(gpu);
    }

    /// Finish the stroke, consuming the engine and returning the record.
    pub fn end(self) -> StrokeRecord {
        self.record
    }

    /// Number of dabs placed so far.
    pub fn dab_count(&self) -> u32 {
        self.dab_count
    }
}

/// Per-dab motion: delta from the previous emitted dab. `tracker` is the
/// position of the most recently emitted dab, or `None` at stroke start /
/// after a rewind. Returns `[0, 0]` when there is no previous dab — that's
/// the contract smudge relies on (zero motion → identity smear write).
fn advance_dab_motion(tracker: &mut Option<[f32; 2]>, pos: [f32; 2]) -> [f32; 2] {
    let motion = match *tracker {
        Some(prev) => [pos[0] - prev[0], pos[1] - prev[1]],
        None => [0.0, 0.0],
    };
    *tracker = Some(pos);
    motion
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: per-dab motion must be the previous-dab → this-dab delta,
    /// not the segment delta. The old bug carried `PaintInformation.motion`
    /// from `derive_sensors` (event-to-event) through to every interpolated
    /// dab in the segment, so a 100px segment with 20 dabs at 5px spacing
    /// would seed `motion=[100,0]` for every dab — wrong for smudge. After
    /// the fix, each dab sees its own ~5px step.
    #[test]
    fn motion_is_per_dab_delta_not_segment_delta() {
        let mut tracker: Option<[f32; 2]> = None;

        // First dab — no prior dab, motion must be zero.
        assert_eq!(advance_dab_motion(&mut tracker, [0.0, 0.0]), [0.0, 0.0]);

        // 20 dabs at 5px spacing along x — each motion must be ~5px, not 100px.
        for i in 1..=20 {
            let pos = [i as f32 * 5.0, 0.0];
            let m = advance_dab_motion(&mut tracker, pos);
            assert!(
                (m[0] - 5.0).abs() < 1e-6 && m[1].abs() < 1e-6,
                "dab {i}: expected ~[5,0], got {m:?} (regression: per-segment motion leaking through)"
            );
        }
    }

    #[test]
    fn motion_resets_to_zero_after_rewind() {
        let mut tracker: Option<[f32; 2]> = None;
        advance_dab_motion(&mut tracker, [10.0, 10.0]);
        advance_dab_motion(&mut tracker, [20.0, 10.0]);
        // Simulate `reset_render_state` clearing the tracker.
        tracker = None;
        assert_eq!(advance_dab_motion(&mut tracker, [100.0, 100.0]), [0.0, 0.0]);
    }

    #[test]
    fn motion_diagonal_step() {
        let mut tracker: Option<[f32; 2]> = None;
        advance_dab_motion(&mut tracker, [10.0, 20.0]);
        let m = advance_dab_motion(&mut tracker, [13.0, 24.0]);
        assert!((m[0] - 3.0).abs() < 1e-6 && (m[1] - 4.0).abs() < 1e-6);
    }
}
