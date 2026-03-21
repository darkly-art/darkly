//! Stroke engine — bridges pen input events to the brush node graph.
//!
//! Owns the `BrushGraphRunner` for the stroke duration and handles:
//! - Storing raw events in `StrokeRecord` (for re-rendering)
//! - Position smoothing (weighted moving average)
//! - Computing derived sensor values (speed, distance, angle, tilt)
//! - Interpolating between events and placing dabs at spacing intervals
//! - Evaluating the brush graph per dab (CPU + GPU)

use super::dab_pool::MAX_DAB_SIZE;
use super::eval::BrushGraphRunner;
use super::gpu_context::BrushGpuContext;
use super::interpolation::lerp_paint_info;
use super::paint_info::{PaintInformation, StrokeRecord};
use super::spacing::SpacingConfig;

/// Reference maximum speed in px/sec for normalizing speed to 0-1.
const MAX_SPEED_PX_PER_SEC: f32 = 4000.0;

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

    /// Smoothing weight (0 = no smoothing, 0.5 = moderate, 0.9 = heavy).
    smoothing_weight: f32,
    /// Smoothed position (weighted moving average).
    smoothed_pos: [f32; 2],

    /// Last processed (post-smoothing, post-derived) point for interpolation.
    last_point: Option<PaintInformation>,
    /// Cumulative distance along the stroke path (in pixels).
    accumulated_distance: f32,
    /// Distance remaining from the last move_to that didn't reach the next
    /// spacing threshold — carried forward to the next move_to.
    leftover_distance: f32,
    /// Dab size [w, h] from the last evaluated dab (for spacing and bounding rect).
    last_dab_size: [f32; 2],
    /// Running dab index within the stroke.
    dab_count: u32,

    /// Time of the first event (seconds), for normalizing time to stroke-relative.
    stroke_start_time: Option<f32>,

    /// Bounding rect of all dabs placed: [x, y, w, h]. None until first dab.
    stroke_rect: Option<[u32; 4]>,

    /// Per-stroke random value (0-1), constant across all dabs.
    fuzzy_stroke: f32,
    /// Stroke seed for deterministic per-dab randomness.
    stroke_seed: u32,
}

impl StrokeEngine {
    /// Create a new stroke engine.
    ///
    /// `runner` is a pre-compiled brush graph. `color` is the foreground
    /// color (linear RGBA). `spacing` and `smoothing_weight` control dab
    /// placement behavior.
    pub fn new(
        runner: BrushGraphRunner,
        color: [f32; 4],
        spacing: SpacingConfig,
        smoothing_weight: f32,
    ) -> Self {
        // Generate stroke seed from system time for deterministic PRNG.
        // Uses web-time which is a drop-in replacement that works on WASM.
        let stroke_seed = web_time::SystemTime::now()
            .duration_since(web_time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u32)
            .unwrap_or(42);
        let fuzzy_stroke = Self::prng_f32(stroke_seed, 0);

        let d = Self::default_diameter();
        Self {
            runner,
            record: StrokeRecord::new(color, "default".into()),
            spacing,
            smoothing_weight: smoothing_weight.clamp(0.0, 0.95),
            smoothed_pos: [0.0; 2],
            last_point: None,
            accumulated_distance: 0.0,
            leftover_distance: 0.0,
            last_dab_size: [d, d],
            dab_count: 0,
            stroke_start_time: None,
            stroke_rect: None,
            fuzzy_stroke,
            stroke_seed,
        }
    }

    /// Deterministic PRNG: hash seed + index to produce a 0-1 float.
    /// Uses a simple xorshift-style hash for speed.
    fn prng_f32(seed: u32, index: u32) -> f32 {
        let mut h = seed.wrapping_add(index.wrapping_mul(2654435761));
        h ^= h >> 16;
        h = h.wrapping_mul(0x45d9f3b);
        h ^= h >> 16;
        h = h.wrapping_mul(0x45d9f3b);
        h ^= h >> 16;
        (h & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
    }

    /// Default dab diameter for initial spacing (before the first dab is evaluated).
    /// Based on the procedural node's default size (0.5) → radius = 0.5 * 256 = 128 → diameter ≈ 258.
    fn default_diameter() -> f32 {
        MAX_DAB_SIZE as f32 * 0.5
    }

    /// The effective canvas-space diameter for spacing and bounding rect,
    /// accounting for global_scale.
    fn effective_diameter(&self, global_scale: f32) -> f32 {
        self.last_dab_size[0].max(self.last_dab_size[1]) * global_scale
    }

    /// Process a raw pointer event — store, smooth, derive, interpolate, and
    /// place dabs along the path.
    ///
    /// `raw` contains the tablet data for this event.  `gpu` provides
    /// everything needed to record GPU render passes for dab generation
    /// and compositing.
    ///
    /// All dab render passes for this move_to are recorded into `gpu.encoder`.
    /// The caller submits the encoder after this returns.
    pub fn move_to(&mut self, raw: PaintInformation, gpu: &mut BrushGpuContext) {
        // 1. Store raw event (pre-smoothing) for replay capability.
        self.record.push(raw);

        // 2. Normalize time relative to stroke start.
        let time = if let Some(start) = self.stroke_start_time {
            raw.time - start
        } else {
            self.stroke_start_time = Some(raw.time);
            0.0
        };

        // 3. Apply position smoothing.
        let smoothed_pos = if self.last_point.is_some() {
            let w = self.smoothing_weight;
            [
                self.smoothed_pos[0] * w + raw.pos[0] * (1.0 - w),
                self.smoothed_pos[1] * w + raw.pos[1] * (1.0 - w),
            ]
        } else {
            raw.pos
        };
        self.smoothed_pos = smoothed_pos;

        // 4. Compute derived values.
        let mut info = raw;
        info.pos = smoothed_pos;
        info.time = time;
        info.index = self.record.len() as u32 - 1;

        // Tilt derived values.
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

        // 5. Place dabs.
        if self.last_point.is_none() {
            // First event: place one dab at the initial position.
            self.place_dab(&info, gpu);
            self.last_point = Some(info);
            return;
        }

        let prev = self.last_point.unwrap();
        let dx = info.pos[0] - prev.pos[0];
        let dy = info.pos[1] - prev.pos[1];
        let segment_dist = (dx * dx + dy * dy).sqrt();

        if segment_dist < 0.001 {
            // No movement — skip.
            self.last_point = Some(info);
            return;
        }

        // Walk along the segment, placing dabs at spacing intervals.
        let mut traveled = self.leftover_distance;

        while traveled < segment_dist {
            let t = traveled / segment_dist;
            let dab_info = lerp_paint_info(&prev, &info, t);
            self.place_dab(&dab_info, gpu);

            // Recompute spacing after each dab — dynamic size may change it.
            // Use the effective (scaled) diameter for canvas-space spacing.
            traveled += self.spacing.distance(self.effective_diameter(gpu.global_scale));
        }

        // Store leftover for next move_to.
        self.leftover_distance = traveled - segment_dist;
        self.last_point = Some(info);
    }

    /// Evaluate the brush graph for a single dab at the given position.
    fn place_dab(&mut self, info: &PaintInformation, gpu: &mut BrushGpuContext) {
        // Set per-dab randomness and fade sensor.
        let mut dab_info = *info;
        dab_info.fuzzy_dab = Self::prng_f32(self.stroke_seed, self.dab_count);
        dab_info.fuzzy_stroke = self.fuzzy_stroke;
        dab_info.fade = (dab_info.distance / FADE_DISTANCE_PX).min(1.0);

        self.runner.clear_slots();
        self.runner.seed_sensors(&dab_info, self.record.color);
        self.runner.execute_cpu();
        self.runner.execute_gpu(gpu);
        gpu.submit_and_reset();
        gpu.dab_pool.release_all();

        // Update dab size from dab source node output (procedural or stamp).
        // Try both node types — only one will be present in a given graph.
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

        // Expand bounding rect using scaled diameter.
        let eff_d = self.effective_diameter(gpu.global_scale);
        let radius = eff_d * 0.5 + 2.0;
        self.expand_rect(info.pos[0], info.pos[1], radius, gpu.canvas_width, gpu.canvas_height);

        self.dab_count += 1;
    }

    /// Expand the stroke bounding rect to include a circle.
    fn expand_rect(&mut self, cx: f32, cy: f32, radius: f32, canvas_w: u32, canvas_h: u32) {
        let x0 = (cx - radius).max(0.0) as u32;
        let y0 = (cy - radius).max(0.0) as u32;
        let x1 = ((cx + radius).ceil() as u32).min(canvas_w);
        let y1 = ((cy + radius).ceil() as u32).min(canvas_h);

        self.stroke_rect = Some(match self.stroke_rect {
            None => [x0, y0, x1 - x0, y1 - y0],
            Some([sx, sy, sw, sh]) => {
                let nx = sx.min(x0);
                let ny = sy.min(y0);
                let nx1 = (sx + sw).max(x1);
                let ny1 = (sy + sh).max(y1);
                [nx, ny, nx1 - nx, ny1 - ny]
            }
        });
    }

    /// Finish the stroke, consuming the engine and returning the record.
    pub fn end(self) -> (StrokeRecord, Option<[u32; 4]>) {
        (self.record, self.stroke_rect)
    }

    /// Replay a stroke record through this engine (for re-rendering).
    ///
    /// This is a skeleton — full re-rendering support is Phase 7e.
    pub fn replay(
        &mut self,
        record: &StrokeRecord,
        gpu: &mut BrushGpuContext,
    ) {
        self.record = StrokeRecord::new(record.color, record.brush_graph_id.clone());
        self.last_point = None;
        self.accumulated_distance = 0.0;
        self.leftover_distance = 0.0;
        let d = Self::default_diameter();
        self.last_dab_size = [d, d];
        self.dab_count = 0;
        self.stroke_start_time = None;
        self.stroke_rect = None;
        self.smoothed_pos = [0.0; 2];
        // Preserve stroke_seed and fuzzy_stroke for deterministic replay.

        for event in &record.events {
            self.move_to(*event, gpu);
        }
    }

    /// The accumulated bounding rect (for undo region tracking).
    pub fn stroke_rect(&self) -> Option<[u32; 4]> {
        self.stroke_rect
    }

    /// Number of dabs placed so far.
    pub fn dab_count(&self) -> u32 {
        self.dab_count
    }
}
