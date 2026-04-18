//! Stroke engine — bridges pen input events to the brush node graph.
//!
//! Owns the `BrushGraphRunner` for the stroke duration and handles:
//! - Storing raw events in `StrokeRecord` (for re-rendering)
//! - Stabilization (retroactive stroke reshaping via pluggable algorithm)
//! - Computing derived sensor values (speed, distance, angle, tilt)
//! - Interpolating between events and placing dabs at spacing intervals
//! - Evaluating the brush graph per dab (CPU + GPU)
//! - Per-dab save points for rewind capability

use super::dab_pool::MAX_DAB_SIZE;
use super::eval::BrushGraphRunner;
use super::gpu_context::BrushGpuContext;
use super::interpolation::lerp_paint_info;
use super::paint_info::{PaintInformation, StrokeRecord};
use super::save_points::SavePointStore;
use super::spacing::SpacingConfig;
use super::stabilizer::{StabilizerAlgorithm, StabilizeResult};

/// Reference maximum speed in px/sec for normalizing speed to 0-1.
const MAX_SPEED_PX_PER_SEC: f32 = 4000.0;

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
            dab_count: 0,
            stroke_seed,
        }
    }

    /// Default dab diameter for initial spacing (before the first dab is evaluated).
    fn default_diameter() -> f32 {
        MAX_DAB_SIZE as f32 * 0.5
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
            dab_count: self.dab_count,
        }
    }

    /// Restore render state from a checkpoint.
    pub fn restore_render_state(&mut self, checkpoint: &RenderCheckpoint) {
        self.last_point = checkpoint.last_point;
        self.accumulated_distance = checkpoint.accumulated_distance;
        self.leftover_distance = checkpoint.leftover_distance;
        self.last_dab_size = checkpoint.last_dab_size;
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
        self.dab_count = 0;
        self.save_points.clear();
    }

    /// Render dabs along the stabilized polyline starting from `start_vector_index`.
    ///
    /// Used for partial re-render after checkpoint restoration. Walks the
    /// stabilized polyline from `start_vector_index` to tip, computing derived
    /// values (speed, distance, angle) between consecutive points, and
    /// placing dabs at spacing intervals.
    pub fn render_from_stabilized_range(&mut self, gpu: &mut BrushGpuContext, start_vector_index: usize) {
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

            // Compute derived values from the stabilized positions.
            info.tilt_magnitude = (info.x_tilt * info.x_tilt + info.y_tilt * info.y_tilt).sqrt().min(1.0);
            info.tilt_direction = info.y_tilt.atan2(info.x_tilt);

            if let Some(ref prev) = self.last_point {
                let dx = info.pos[0] - prev.pos[0];
                let dy = info.pos[1] - prev.pos[1];
                let dist = (dx * dx + dy * dy).sqrt();

                self.accumulated_distance += dist;
                info.distance = self.accumulated_distance;
                info.drawing_angle = dy.atan2(dx);

                let dt = info.time - prev.time;
                if dt > 0.0 {
                    let speed_px_per_sec = dist / dt;
                    info.speed = (speed_px_per_sec / MAX_SPEED_PX_PER_SEC).min(1.0);
                } else {
                    info.speed = prev.speed;
                }
            }

            // Place dabs along the segment from last_point to info.
            if self.last_point.is_none() {
                self.place_dab(&info, gpu, i);
                self.last_point = Some(info);
                self.save_points.finalize_render_state(i, self.capture_render_state());
                continue;
            }

            let prev = self.last_point.unwrap();
            let dx = info.pos[0] - prev.pos[0];
            let dy = info.pos[1] - prev.pos[1];
            let segment_dist = (dx * dx + dy * dy).sqrt();

            if segment_dist < 0.001 {
                self.last_point = Some(info);
                continue;
            }

            let mut traveled = self.leftover_distance;
            while traveled < segment_dist {
                let t = traveled / segment_dist;
                let dab_info = lerp_paint_info(&prev, &info, t);
                self.place_dab(&dab_info, gpu, i);
                traveled += self.spacing.distance(self.effective_diameter());
            }

            self.leftover_distance = traveled - segment_dist;
            self.last_point = Some(info);

            // Capture end-of-segment state on ALL save points for this vector
            // index.  This represents "everything through vector index i is
            // fully processed" — the checkpoint restore starts from i+1.
            self.save_points.finalize_render_state(i, self.capture_render_state());
        }
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
    fn place_dab(&mut self, info: &PaintInformation, gpu: &mut BrushGpuContext, vector_index: usize) {
        let mut dab_info = *info;
        dab_info.fade = (dab_info.distance / FADE_DISTANCE_PX).min(1.0);

        self.runner.clear_slots();
        self.runner.seed_sensors(
            &dab_info,
            self.record.color,
            self.stroke_seed,
            self.dab_count,
        );
        self.runner.execute_cpu();
        self.runner.execute_gpu(gpu);
        gpu.dab_pool.release_all();
        gpu.flush_if_needed();

        // Update dab size from dab source node output (procedural or stamp).
        for node_type in &["procedural", "stamp"] {
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

        // Compute dab bounding box for save points.
        let diameter = self.effective_diameter();
        let half = diameter * 0.5;
        let x = (info.pos[0] - half).max(0.0) as u32;
        let y = (info.pos[1] - half).max(0.0) as u32;
        let x2 = (info.pos[0] + half).ceil() as u32;
        let y2 = (info.pos[1] + half).ceil() as u32;
        let w = x2.saturating_sub(x);
        let h = y2.saturating_sub(y);
        // Render state is captured at end-of-segment, not per-dab.
        // Push a placeholder; the loop in render_from_stabilized_range
        // overwrites the last save point's render_state after each segment.
        self.save_points.push([x, y, w, h], vector_index, RenderCheckpoint {
            last_point: None,
            accumulated_distance: 0.0,
            leftover_distance: 0.0,
            last_dab_size: [0.0, 0.0],
            dab_count: 0,
        });

        self.dab_count += 1;
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

        info.tilt_magnitude = (info.x_tilt * info.x_tilt + info.y_tilt * info.y_tilt).sqrt().min(1.0);
        info.tilt_direction = info.y_tilt.atan2(info.x_tilt);

        if let Some(ref prev) = self.last_point {
            let dx = info.pos[0] - prev.pos[0];
            let dy = info.pos[1] - prev.pos[1];
            let dist = (dx * dx + dy * dy).sqrt();

            self.accumulated_distance += dist;
            info.distance = self.accumulated_distance;
            info.drawing_angle = dy.atan2(dx);

            let dt = info.time - prev.time;
            if dt > 0.0 {
                let speed_px_per_sec = dist / dt;
                info.speed = (speed_px_per_sec / MAX_SPEED_PX_PER_SEC).min(1.0);
            } else {
                info.speed = prev.speed;
            }
        }

        if self.last_point.is_none() {
            self.place_dab(&info, gpu, len - 1);
            self.last_point = Some(info);
            self.save_points.finalize_render_state(len - 1, self.capture_render_state());
            return;
        }

        let prev = self.last_point.unwrap();
        let dx = info.pos[0] - prev.pos[0];
        let dy = info.pos[1] - prev.pos[1];
        let segment_dist = (dx * dx + dy * dy).sqrt();

        if segment_dist < 0.001 {
            self.last_point = Some(info);
            return;
        }

        let mut traveled = self.leftover_distance;
        while traveled < segment_dist {
            let t = traveled / segment_dist;
            let dab_info = lerp_paint_info(&prev, &info, t);
            self.place_dab(&dab_info, gpu, len - 1);
            traveled += self.spacing.distance(self.effective_diameter());
        }

        self.leftover_distance = traveled - segment_dist;
        self.last_point = Some(info);
        self.save_points.finalize_render_state(len - 1, self.capture_render_state());
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
