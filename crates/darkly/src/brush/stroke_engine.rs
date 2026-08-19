//! Stroke engine — bridges pen input events to the brush node graph.
//!
//! Owns the `BrushGraphRunner` for the stroke duration and handles:
//! - Storing raw events in `StrokeRecord` (for re-rendering)
//! - Stabilization (retroactive stroke reshaping via pluggable algorithm)
//! - Computing derived sensor values (speed, distance, angle, tilt)
//! - Interpolating between events and placing dabs at spacing intervals
//! - Evaluating the brush graph per dab (CPU + GPU)
//! - Per-dab save points for rewind capability

use super::eval::BrushGraphRunner;
use super::gpu_context::BrushGpuContext;
use super::interpolation::{lerp_paint_info, CatmullRomSegment};
use super::paint_info::{PaintInformation, StrokeRecord};
use super::save_points::SavePointStore;
use super::spacing::SpacingConfig;
use super::stabilizer::{StabilizeResult, StabilizerAlgorithm};
use super::DAB_REFERENCE_SIZE;

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
    pub stamp_angle: Option<f32>,
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

    /// Held stamp orientation (canvas-frame radians) — the stroke axis the
    /// dab is currently facing, as opposed to the instantaneous travel
    /// direction. `None` until the first dab that has actually travelled.
    /// Reset at stroke start and on full re-render; carried across a partial
    /// re-render on [`RenderCheckpoint`] so the seam is continuous.
    stamp_angle: Option<f32>,
    /// How far `stamp_angle` may turn per brush diameter of travel (radians).
    /// Stroke-constant, read from `brush_settings` at stroke start.
    stamp_angle_rate: f32,

    /// Stroke seed for deterministic per-dab randomness.  Passed to
    /// the runner so random nodes can generate independent sequences.
    stroke_seed: u32,

    /// Clone set-source anchor (plane / canvas pixels), or `None` for a
    /// non-clone brush. Combined with `clone_dest_anchor` into the
    /// runner's [`CloneState`] each dab so the `clone_source` node's
    /// anchor uniforms are seeded.
    clone_source_anchor: Option<[f32; 2]>,
    /// Destination anchor — the position of the stroke's first rendered
    /// dab. Captured lazily in `place_dab` (the stabilizer offsets the
    /// first dab, so raw engine input is wrong); reset on full re-render.
    clone_dest_anchor: Option<[f32; 2]>,
    /// Plane-space frame of the clone source snapshot, refreshed by the
    /// engine every pen event via [`Self::set_clone_source_frame`] (the
    /// frozen cross-layer / merged snapshot's rect when one exists, else
    /// the paint target's current extent so same-layer clone tracks
    /// mid-stroke layer growth). Stroke-stable: NOT cleared by
    /// [`Self::reset_render_state`] — divergence rewind reuses it.
    clone_source_frame: Option<crate::coord::CanvasRect>,
}

impl StrokeEngine {
    /// Create a new stroke engine.
    ///
    /// `runner` is a pre-compiled brush graph.  `color` is the foreground
    /// color (raw sRGB RGBA, as picked).  `spacing` controls dab placement.
    /// `stabilizer` is the stroke stabilization algorithm.  `stamp_angle_rate`
    /// caps how fast the stamp pivots to follow the stroke, in radians per
    /// brush diameter of travel.  `stroke_seed` drives every `random`/`noise`
    /// node in the graph — a real stroke passes [`Self::random_seed`], a render
    /// that has to be reproducible passes a constant.
    pub fn new(
        mut runner: BrushGraphRunner,
        color: [f32; 4],
        spacing: SpacingConfig,
        base_size: f32,
        stabilizer: Box<dyn StabilizerAlgorithm>,
        clone_source_anchor: Option<[f32; 2]>,
        stroke_seed: u32,
        stamp_angle_rate: f32,
    ) -> Self {
        // Base brush size is stroke-constant, read out-of-band from
        // `pen_input.size` at stroke start. Injected as ambient state so every
        // terminal's `effective_radius` and the `pen_input.size` graph signal
        // see one consistent value.
        runner.set_base_size(base_size);

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
            stamp_angle: None,
            stamp_angle_rate,
            stroke_seed,
            clone_source_anchor,
            clone_dest_anchor: None,
            clone_source_frame: None,
        }
    }

    /// A seed drawn from the wall clock, so two strokes of the same brush
    /// scatter differently. What a stroke the painter is making wants — and
    /// what a stroke rendered into a cached thumbnail or a documentation asset
    /// must not have, which is why it is the caller's to choose.
    /// Texel format the stroke scratch must be allocated in for this
    /// stroke's brush — see
    /// [`BrushGraphRunner::scratch_format`](crate::brush::eval::BrushGraphRunner::scratch_format).
    /// The engine builds its `StrokeEngine` before its `StrokeBuffer`, so
    /// this is available at allocation time.
    pub fn scratch_format(&self) -> wgpu::TextureFormat {
        self.runner.scratch_format()
    }

    pub fn random_seed() -> u32 {
        web_time::SystemTime::now()
            .duration_since(web_time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u32)
            .unwrap_or(42)
    }

    /// Set the clone source snapshot's plane-space frame for the current
    /// stroke. Called by the engine every pen event, before rendering —
    /// see the field doc for what the frame is.
    pub fn set_clone_source_frame(&mut self, frame: crate::coord::CanvasRect) {
        self.clone_source_frame = Some(frame);
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
            stamp_angle: self.stamp_angle,
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
        self.stamp_angle = checkpoint.stamp_angle;
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
        // Re-seeded from the first travelling dab of the re-render. The rate
        // itself is stroke-constant configuration and survives.
        self.stamp_angle = None;
        // Recapture the destination anchor from the re-stabilized first
        // dab on the next `place_dab`.
        self.clone_dest_anchor = None;
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

    /// Advance the held stamp orientation for a dab travelling `travel` canvas
    /// pixels in direction `direction`, at brush diameter `diameter`. Thin
    /// wrapper over the free function, mirroring [`Self::next_dab_motion`], so
    /// the orientation contract is unit-testable without a GPU.
    fn next_stamp_angle(&mut self, direction: f32, travel: f32, diameter: f32) -> f32 {
        advance_stamp_angle(
            &mut self.stamp_angle,
            direction,
            travel,
            diameter,
            self.stamp_angle_rate,
        )
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
        // `stab_len` is cached once: nothing inside the loop mutates the
        // stabilizer, so the count can't drift. We then scope each
        // `self.stabilizer.stabilized()` borrow tightly — copying the
        // handful of `PaintInformation` values we need (it's `Copy`) and
        // releasing the slice before calling `self.place_dab`, which
        // takes `&mut self`. This replaces the prior full-polyline
        // `.to_vec()` clone (one alloc per `render_from_*` call, growing
        // linearly with stroke length).
        let stab_len = self.stabilizer.len();
        if stab_len == 0 {
            return;
        }

        let start = start_vector_index.min(stab_len);
        let end = end_vector_index.min(stab_len - 1);

        // When resuming from a checkpoint, snap last_point.pos to the current
        // stabilized position.  Between checkpoint capture and now, intermediate
        // frames may have shifted the polyline — the checkpoint's last_point
        // reflects the old position.  Without this, the first segment bridges
        // from the old position to the new next point, creating a tangent
        // discontinuity ("broken chain" artifact at corners).
        if start > 0 {
            let snap_pos = self.stabilizer.stabilized().get(start - 1).map(|p| p.pos);
            if let (Some(pos), Some(lp)) = (snap_pos, self.last_point.as_mut()) {
                lp.pos = pos;
            }
        }

        // Walk the polyline, computing derived values and placing dabs.
        for i in start..=end {
            let (raw, prev_neighbor, next_neighbor) = {
                let stab = self.stabilizer.stabilized();
                let raw = stab[i];
                let prev = if i >= 2 { Some(stab[i - 2]) } else { None };
                let next = if i + 1 < stab.len() {
                    Some(stab[i + 1])
                } else {
                    None
                };
                (raw, prev, next)
            };
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
            let p0_pt = prev_neighbor.unwrap_or(prev);
            let p1_pt = prev;
            let p2_pt = info;
            let p3_pt = next_neighbor.unwrap_or(info);

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
        // Stamp orientation is likewise per-dab and order-dependent: the stamp
        // pivots as the brush travels, toward the stroke's undirected axis and
        // no faster than the brush's turn rate. Runs after interpolation (the
        // caller interpolates before every `place_dab`), so it is the last
        // transform on the angle before the graph sees it.
        let travel = dab_info.motion[0].hypot(dab_info.motion[1]);
        let diameter = self.effective_diameter();
        dab_info.drawing_angle = self.next_stamp_angle(dab_info.drawing_angle, travel, diameter);

        // Clone uniforms: capture the destination at the first rendered
        // dab (post-stabilization), then seed the runner's CloneState so
        // the `clone_source` node's uniforms carry the anchors and the
        // source frame. No-op for non-clone brushes (`clone_source_anchor`
        // is `None`).
        if let Some(source_anchor) = self.clone_source_anchor {
            let dest_anchor = *self.clone_dest_anchor.get_or_insert(dab_info.pos);
            // The engine refreshes the frame every pen event before any
            // dab is placed; the fallback identity frame only guards a
            // driver that forgot to (and would sample garbage UVs anyway).
            debug_assert!(
                self.clone_source_frame.is_some(),
                "clone stroke rendered without set_clone_source_frame"
            );
            let (source_offset, source_size) = match self.clone_source_frame {
                Some(f) => (
                    [f.x0() as f32, f.y0() as f32],
                    [f.width as f32, f.height as f32],
                ),
                None => ([0.0, 0.0], [1.0, 1.0]),
            };
            self.runner.set_clone_state(Some(super::eval::CloneState {
                source_anchor,
                dest_anchor,
                source_offset,
                source_size,
            }));
        }

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
        if let Some(stroke) = gpu.stroke.as_mut() {
            stroke.reset_per_dab_read_cache();
        }
        // Reset the write-bbox accumulator so each terminal's passes can
        // publish their footprint fresh. Read back after execute_gpu below.
        gpu.dab_batch.write_canvas_bbox = None;
        // Queue depth before the terminal runs — a dab that lands in the
        // queue but publishes no footprint is a programming error, caught
        // by the debug-assert below.
        let queued_before = gpu.dab_batch.count;
        self.runner.execute_gpu(gpu);

        gpu.flush_if_needed();

        // Update `last_dab_size` from whichever terminal in the graph
        // publishes a `dab_size` output. The runner cached the slot at
        // build time, so a new terminal that publishes the same port is
        // picked up automatically — no hand-written terminal-name list
        // to keep in sync.
        if let Some(size) = self.runner.last_dab_size() {
            self.last_dab_size = size;
        }

        // Dab bounding box for save points, in canvas coords: the footprint
        // the terminal published for the pass it issued (post-scatter,
        // post-anything else the graph did). A dab that wrote nothing — zero
        // diameter, entirely off-extent, an identity-transform early-out —
        // publishes nothing and records an empty rect, which unions away.
        //
        // There is deliberately no geometric fallback here. An envelope
        // derived from `pos ± radius` omits the compiled brush's extent
        // inflation, so it can bound the checkpoint more tightly than the
        // shader writes — and a rewind then clears pixels it cannot restore.
        // See `ExtentContribution`'s doc comment for the shipped instance of
        // that bug.
        let canvas_bbox = gpu
            .dab_batch
            .write_canvas_bbox
            .unwrap_or(crate::coord::CanvasRect::from_xywh(0, 0, 0, 0));
        debug_assert!(
            gpu.dab_batch.count == queued_before || !canvas_bbox.is_empty(),
            "terminal queued a dab without publishing its write footprint; \
             the save-point bbox would miss pixels the shader writes",
        );
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
                stamp_angle: None,
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
            // Terminals queue the dab and rely on `flush_dabs` to
            // actually run the render pass. Without this, a single-
            // event stroke (one `move_to` + `end_stroke`) leaves the
            // queued first dab unflushed and the stroke renders as
            // nothing.
            self.runner.flush_dabs(gpu);
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

/// Advance the held stamp orientation one dab and return what the dab should
/// face.
///
/// `held` carries the orientation from the previous emitted dab. It is `None`
/// at stroke start and after a full re-render; while it is `None` and `travel`
/// is zero the direction passes through untouched and nothing is adopted,
/// because a dab that has not travelled has no measured direction to adopt
/// (`PaintInformation::derive_sensors` leaves a stroke's first `drawing_angle`
/// at its default of zero). The first dab that has travelled seeds `held`.
///
/// `direction` is the dab's signed travel angle, `travel` the canvas-pixel
/// distance from the previous dab, `diameter` the brush's effective canvas
/// diameter, and `rate` the permitted turn in radians per diameter of travel —
/// or [`STAMP_ANGLE_RATE_UNLIMITED`], at which the cap is skipped entirely and
/// only the fold applies.
///
/// The axis fold — taking whichever of `direction` / `direction + π` is nearer
/// to the held orientation — is unconditional. A symmetric stamp is identical
/// at both, so reversing along a stroke must not spin it a half turn.
///
/// [`STAMP_ANGLE_RATE_UNLIMITED`]: crate::brush::nodes::brush_settings::STAMP_ANGLE_RATE_UNLIMITED
fn advance_stamp_angle(
    held: &mut Option<f32>,
    direction: f32,
    travel: f32,
    diameter: f32,
    rate: f32,
) -> f32 {
    use crate::brush::interpolation::shortest_angle_diff;
    use crate::brush::nodes::brush_settings::STAMP_ANGLE_RATE_UNLIMITED;
    use std::f32::consts::{FRAC_PI_2, PI};

    let Some(phi) = *held else {
        if travel <= 0.0 {
            return direction;
        }
        *held = Some(direction);
        return direction;
    };

    // Fold to the nearer of the two representatives of the same axis, so a
    // direction reversal costs no rotation at all.
    let mut d = shortest_angle_diff(phi, direction);
    if d.abs() > FRAC_PI_2 {
        d -= d.signum() * PI;
    }

    // A turn rate per unit of travel: zero travel permits zero rotation, so a
    // stationary pen cannot spin the stamp. `max(diameter, 1.0)` keeps the
    // division defined if a terminal ever publishes a degenerate dab size.
    if rate < STAMP_ANGLE_RATE_UNLIMITED {
        let allowed = rate * travel / diameter.max(1.0);
        d = d.clamp(-allowed, allowed);
    }

    // Wrapped each dab so a long stroke can't drift the magnitude upward; the
    // value only ever reaches `cos`/`sin` downstream, so this is invisible.
    let next = shortest_angle_diff(0.0, phi + d);
    *held = Some(next);
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::interpolation::shortest_angle_diff;

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

    // ── Stamp orientation tracker ───────────────────────────────────────

    /// Diameter and per-dab travel used by the orientation tests: a 40 px
    /// brush stepping 4 px per dab, i.e. the default 10% spacing.
    const D: f32 = 40.0;
    const STEP: f32 = 4.0;

    /// Rate that permits a quarter turn per dab at the constants above, so a
    /// test that wants the cap out of the way can say so without reaching for
    /// the sentinel.
    const LOOSE_RATE: f32 = std::f32::consts::FRAC_PI_2 * D / STEP;

    fn feed(held: &mut Option<f32>, direction: f32, rate: f32) -> f32 {
        advance_stamp_angle(held, direction, STEP, D, rate)
    }

    /// The stroke axis is undirected: reversing direction must not spin a
    /// symmetric stamp a half turn. This is the fold, and it holds at any rate.
    #[test]
    fn reversal_folds_to_axis_without_half_turn() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let mut held = None;
        for _ in 0..5 {
            feed(&mut held, 0.0, LOOSE_RATE);
        }
        let before = held.unwrap();

        let after = feed(&mut held, PI, LOOSE_RATE);
        assert!(
            shortest_angle_diff(before, after).abs() <= FRAC_PI_2 + 1e-5,
            "a reversal must not rotate the stamp more than a quarter turn; \
             went from {before} to {after}"
        );
        assert!(
            after.abs() < 1e-4,
            "after reversing, the stamp should still lie on the original axis \
             (near 0), not near π; got {after}"
        );
    }

    /// The load-bearing invariant: the cap is per unit of *travel*, so the
    /// same geometric turn over the same total distance ends at the same
    /// orientation no matter how finely it is subdivided. A per-dab cap fails
    /// this by the ratio of the two spacings.
    #[test]
    fn rate_is_per_diameter_of_travel_not_per_dab() {
        // A rate tight enough that the cap is the binding constraint in both
        // runs: a quarter turn demanded immediately, far more than allowed.
        let rate = 0.5;
        let target = std::f32::consts::FRAC_PI_2;

        let mut coarse = Some(0.0);
        for _ in 0..10 {
            advance_stamp_angle(&mut coarse, target, 0.1 * D, D, rate);
        }

        let mut fine = Some(0.0);
        for _ in 0..20 {
            advance_stamp_angle(&mut fine, target, 0.05 * D, D, rate);
        }

        // Both travelled 1.0 × D in total.
        let (a, b) = (coarse.unwrap(), fine.unwrap());
        assert!(
            (a - b).abs() < 1e-4,
            "equal total travel must give equal orientation regardless of dab \
             subdivision; coarse={a}, fine={b} (a per-dab cap would differ by ~2x)"
        );
        assert!(
            (a - rate).abs() < 1e-4,
            "after 1.0 diameters of travel at {rate} rad/diameter the stamp \
             should have turned {rate} rad; got {a}"
        );
    }

    /// A cap is a cap, not a smoothing filter: turns comfortably inside the
    /// budget are tracked exactly, with no lag. This is the deliberate
    /// divergence from GIMP's unconditional EMA.
    #[test]
    fn gentle_curve_tracks_without_lag() {
        let mut held = Some(0.0);
        // 1° per dab, against a budget of 5.7° per dab at this rate.
        let per_dab = 1.0_f32.to_radians();
        for i in 1..=30 {
            let target = per_dab * i as f32;
            let got = advance_stamp_angle(&mut held, target, STEP, D, 1.0);
            assert!(
                (got - target).abs() < 1e-5,
                "dab {i}: a turn inside the rate budget must track exactly; \
                 wanted {target}, got {got}"
            );
        }
    }

    /// Zero travel permits zero rotation whenever the cap is engaged — a
    /// stationary pen cannot make the stamp twitch. This is what lets the rate
    /// cap subsume a separate idle-noise filter.
    ///
    /// It is a property of the cap, not of the tracker: at the unlimited
    /// sentinel there is no cap to enforce it, and a stationary dab takes its
    /// angle directly, exactly as it did before the rate limit existed.
    #[test]
    fn zero_travel_cannot_rotate() {
        use crate::brush::nodes::brush_settings::STAMP_ANGLE_RATE_UNLIMITED;

        for rate in [0.0, 0.5, STAMP_ANGLE_RATE_UNLIMITED - 1.0] {
            let mut held = Some(0.0);
            for target in [0.3, -0.7, 1.2, 0.05] {
                let got = advance_stamp_angle(&mut held, target, 0.0, D, rate);
                assert_eq!(
                    got, 0.0,
                    "rate {rate}: a dab that has not travelled must not rotate \
                     the stamp"
                );
            }
        }
    }

    /// The bottom of the range locks the stamp to the angle it started at.
    #[test]
    fn zero_rate_locks_orientation() {
        let mut held = None;
        let start = feed(&mut held, 0.4, 0.0);
        assert!((start - 0.4).abs() < 1e-6);
        for target in [1.0, -1.0, 2.5] {
            let got = feed(&mut held, target, 0.0);
            assert!(
                (got - 0.4).abs() < 1e-6,
                "rate 0 must freeze the orientation; got {got}"
            );
        }
    }

    /// The top of the range is a sentinel meaning *unlimited*, and it is the
    /// shipped default — so this guards the promise that a brush which never
    /// touches the knob is unaffected by the rate limit.
    #[test]
    fn unlimited_rate_skips_the_cap() {
        use crate::brush::nodes::brush_settings::STAMP_ANGLE_RATE_UNLIMITED;
        let mut held = None;
        feed(&mut held, 0.0, STAMP_ANGLE_RATE_UNLIMITED);

        // A near-quarter-turn demanded over a sliver of travel: any finite rate
        // at this travel would clamp it hard.
        let got = advance_stamp_angle(&mut held, 1.5, 0.001, D, STAMP_ANGLE_RATE_UNLIMITED);
        assert!(
            (got - 1.5).abs() < 1e-5,
            "at the unlimited sentinel the stamp must reach the folded target \
             in one dab; got {got}"
        );
    }

    /// A stroke's first point has no segment behind it, so `derive_sensors`
    /// leaves its `drawing_angle` at the default 0 — see
    /// `tests/paint_info_derive_sensors.rs`. Adopting that would point every
    /// stroke rightward at birth and then rate-limit the recovery.
    #[test]
    fn stroke_start_does_not_adopt_zero() {
        use std::f32::consts::FRAC_PI_2;
        let mut held = None;

        // The stroke's first dab: no travel, and a meaningless angle.
        let first = advance_stamp_angle(&mut held, 0.0, 0.0, D, 0.5);
        assert_eq!(first, 0.0, "the first dab passes its angle through");
        assert!(
            held.is_none(),
            "nothing should be adopted from a dab that has not travelled"
        );

        // The first travelling dab establishes the axis outright, with no
        // rate-limited crawl up from 0.
        let second = advance_stamp_angle(&mut held, FRAC_PI_2, STEP, D, 0.5);
        assert!(
            (second - FRAC_PI_2).abs() < 1e-6,
            "the first travelling dab should adopt its direction, not ease \
             toward it from a bogus 0; got {second}"
        );
    }

    /// The cap and the fold compose: a *smooth* turn stays inside the budget,
    /// so the fold never fires and the stamp follows all the way through 180°.
    /// A turn too fast for the budget is allowed to settle on the other axis
    /// representative instead — identical for a symmetric stamp, and the
    /// documented limitation for an asymmetric one.
    #[test]
    fn gradual_u_turn_tracks_without_flipping() {
        use std::f32::consts::PI;

        // 180° over 90 dabs = 2° per dab, well inside a 5.7°/dab budget.
        let mut held = Some(0.0);
        let mut target = 0.0;
        for _ in 0..90 {
            target += 2.0_f32.to_radians();
            advance_stamp_angle(&mut held, target, STEP, D, 1.0);
        }
        let tracked = held.unwrap();
        assert!(
            shortest_angle_diff(PI, tracked).abs() < 1e-3,
            "a gradual U-turn should be followed the whole way to π; got {tracked}"
        );

        // The same 180°, demanded at once under a tight cap: the fold picks
        // the near representative, so the stamp does not move.
        let mut held = Some(0.0);
        let got = advance_stamp_angle(&mut held, PI, STEP, D, 0.01);
        assert!(
            got.abs() < 1e-5,
            "an instant reversal folds to a no-op rather than crawling half a \
             turn; got {got}"
        );
    }
}
