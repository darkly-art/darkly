//! Stroke engine: bridges pen input events to the brush node graph.
//!
//! Owns the `BrushGraphRunner` for the stroke duration and handles:
//! - Storing raw events in `StrokeRecord` (for re-rendering)
//! - Stabilization (retroactive stroke reshaping via pluggable algorithm)
//! - Tracking where each vertex was last rendered, so a render rewinds only
//!   from the earliest vertex that has moved since
//! - Computing derived sensor values (speed, distance, angle, tilt)
//! - Placing dabs at spacing intervals along the straight segment between
//!   consecutive stabilized vertices
//! - Evaluating the brush graph per dab (CPU + GPU)
//! - Per-dab save points for rewind capability

use super::eval::BrushGraphRunner;
use super::gpu_context::{BrushGpuContext, MAX_DABS_PER_PHASE};
use super::interpolation::lerp_paint_info;
use super::paint_info::{PaintInformation, StrokeRecord};
use super::save_points::SavePointStore;
use super::spacing::SpacingConfig;
use super::stabilizer::StabilizerAlgorithm;
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

/// Distance in CSS pixels below which a stabilized point is considered
/// unchanged since it was rendered. The engine scales it to canvas pixels at
/// the stroke's zoom when it builds the [`StrokeEngine`].
///
/// Neighbouring vertices are re-rendered at different moments, so this is
/// also the amplitude of the ripple the rendered stroke can carry that the
/// smoothed polyline does not. A tenth of a pixel keeps that below what a
/// thin antialiased stroke shows; a wider tolerance made quick wide curves
/// look faintly jagged.
pub const DIVERGENCE_EPSILON: f32 = 0.1;

/// Find the earliest rendered index whose position is more than `epsilon`
/// from where it was rendered, looking no further back than `max_window`
/// indices behind the tip as last rendered.
///
/// `current` is the stabilized polyline now, `rendered` the positions its
/// vertices were last rendered at. A stabilizer's window bounds how far
/// behind the tip before a push that push can move a vertex; every push
/// since the last render started at or beyond the rendered tip, so the walk
/// starts `max_window` behind it however many pushes there were. Within the
/// window every index is checked: vertices are re-rendered at different
/// times, so one that sits within tolerance of its own recent render says
/// nothing about the older renders behind it.
///
/// Returns `None` when every rendered index is within tolerance, even if the
/// polyline grew: new indices have never been rendered, so appending them
/// needs no rewind. An `epsilon` of zero reports any change at all.
fn find_divergence(
    current: &[PaintInformation],
    rendered: &[[f32; 2]],
    max_window: usize,
    epsilon: f32,
) -> Option<usize> {
    let overlap = rendered.len().min(current.len());
    let earliest = rendered.len().saturating_sub(max_window + 1);
    let eps2 = epsilon * epsilon;
    let moved = |i: usize| {
        let cur = current[i].pos;
        let was = rendered[i];
        let dx = cur[0] - was[0];
        let dy = cur[1] - was[1];
        let d2 = dx * dx + dy * dy;
        d2 > 0.0 && d2 >= eps2
    };
    debug_assert!(
        !(0..earliest.min(overlap)).any(moved),
        "a vertex more than {max_window} behind the rendered tip moved: \
         the stabilizer's max_divergence_window is not a bound"
    );
    (earliest..overlap).find(|&i| moved(i))
}

/// The positions each vertex of a stabilized polyline was last rendered at,
/// and the diff of the current polyline against them.
///
/// Comparing against rendered positions rather than the previous push keeps
/// every rendered vertex within `epsilon` of where it now lies: a vertex that
/// drifts a little on every push is re-rendered once its accumulated drift
/// reaches `epsilon`, instead of never.
pub struct DivergenceDiff {
    rendered: Vec<[f32; 2]>,
    epsilon: f32,
}

impl DivergenceDiff {
    /// A diff that treats moves shorter than `epsilon` canvas px as unchanged.
    pub fn new(epsilon: f32) -> Self {
        Self {
            rendered: Vec::with_capacity(256),
            epsilon,
        }
    }

    /// Diff `current` against the rendered positions and report the first
    /// index to re-render, recording `current` as rendered from there on.
    /// `max_window` is the stabilizer's bound on how far a push reaches
    /// behind the tip before it.
    pub fn update(&mut self, current: &[PaintInformation], max_window: usize) -> Option<usize> {
        let divergence = find_divergence(current, &self.rendered, max_window, self.epsilon);
        let from = divergence.unwrap_or(self.rendered.len());
        self.rendered.truncate(from);
        self.rendered.extend(current[from..].iter().map(|p| p.pos));
        divergence
    }

    /// Number of vertices recorded as rendered.
    pub fn len(&self) -> usize {
        self.rendered.len()
    }

    /// Whether nothing has been rendered yet.
    pub fn is_empty(&self) -> bool {
        self.rendered.is_empty()
    }
}

/// Reference fade distance in pixels.  The fade sensor goes from 0 to 1
/// over this distance, then clamps at 1.  Configurable per-brush later.
const FADE_DISTANCE_PX: f32 = 1000.0;

/// Drives a single brush stroke from begin to end.
///
/// Created by the engine at stroke start, fed pointer events via
/// [`Self::stabilize`], rendered from the stabilized polyline once per frame
/// (the engine diffs with [`Self::take_divergence`], rewinds, and replays),
/// and consumed at stroke end to yield a `StrokeRecord`.
pub struct StrokeEngine {
    runner: BrushGraphRunner,
    record: StrokeRecord,
    spacing: SpacingConfig,

    /// Pluggable stabilizer algorithm (pass-through when no stabilization).
    stabilizer: Box<dyn StabilizerAlgorithm>,
    /// Where each stabilized vertex was last rendered.
    rendered: DivergenceDiff,
    /// Events of `record` already handed to the stabilizer. Those after it
    /// are pending until the next [`Self::take_divergence`].
    stabilized_events: usize,

    /// Per-dab save points for rewind capability.
    pub save_points: SavePointStore,

    /// Last processed point for interpolation (post-derived-values).
    last_point: Option<PaintInformation>,
    /// Cumulative distance along the stroke path (in pixels).
    accumulated_distance: f32,
    /// Distance remaining from the last segment that didn't reach the next
    /// spacing threshold: carried forward to the next segment.
    leftover_distance: f32,
    /// Dab size [w, h] from the last evaluated dab (for spacing).
    last_dab_size: [f32; 2],
    /// Position of the most recently *emitted* dab: source-of-truth for
    /// `PaintInformation.motion` (per-dab delta, populated in `place_dab`).
    /// Distinct from `last_point` which tracks the previous stabilized
    /// *event*. Reset to `None` at stroke start and on full re-render.
    last_dab_pos: Option<[f32; 2]>,
    /// Running dab index within the stroke.
    dab_count: u32,
    /// Pointer timestamp (ms) of the stroke's first event: the origin of
    /// `PaintInformation.time`. Kept in f64 so inter-sample intervals of a
    /// few ms survive a large absolute page-uptime timestamp.
    time_origin_ms: Option<f64>,

    /// Held stamp orientation (canvas-frame radians): the stroke axis the
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
    /// Destination anchor: the position of the stroke's first rendered
    /// dab. Captured lazily in `place_dab` (the stabilizer offsets the
    /// first dab, so raw engine input is wrong); reset on full re-render.
    clone_dest_anchor: Option<[f32; 2]>,
    /// Plane-space frame of the clone source snapshot, refreshed by the
    /// engine every stroke flush via [`Self::set_clone_source_frame`] (the
    /// frozen cross-layer / merged snapshot's rect when one exists, else
    /// the paint target's current extent so same-layer clone tracks
    /// mid-stroke layer growth). Stroke-stable: NOT cleared by
    /// [`Self::reset_render_state`]; divergence rewind reuses it.
    clone_source_frame: Option<crate::coord::CanvasRect>,
}

impl StrokeEngine {
    /// Create a new stroke engine.
    ///
    /// `runner` is a pre-compiled brush graph.  `color` is the foreground
    /// color (raw sRGB RGBA, as picked).  `spacing` controls dab placement.
    /// `stabilizer` is the stroke stabilization algorithm.  `stamp_angle_rate`
    /// caps how fast the stamp pivots to follow the stroke, in radians per
    /// brush diameter of travel.  `stroke_seed` drives every `random` node in
    /// the graph, reaching them through [`EvalContext::prng_at`]: a real
    /// stroke passes [`Self::random_seed`], a render that has to be
    /// reproducible passes a constant. `divergence_epsilon` is the canvas
    /// distance a rendered vertex may drift before it is re-rendered
    /// ([`DIVERGENCE_EPSILON`] at the stroke's view scale). It does **not** reach `noise`, which
    /// seeds from its own compile-time `seed` port and so is identical from
    /// stroke to stroke.  `dpi` is the owning document's DPI, from which the
    /// runner derives the reference-to-canvas factor; a render with no
    /// document behind it passes [`crate::document::REFERENCE_DPI`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mut runner: BrushGraphRunner,
        color: [f32; 4],
        spacing: SpacingConfig,
        base_size: f32,
        stabilizer: Box<dyn StabilizerAlgorithm>,
        divergence_epsilon: f32,
        clone_source_anchor: Option<[f32; 2]>,
        stroke_seed: u32,
        stamp_angle_rate: f32,
        dpi: f32,
    ) -> Self {
        // Base brush size is stroke-constant, read out-of-band from
        // `pen_input.size` at stroke start. Injected as ambient state so every
        // terminal's `effective_radius` and the `pen_input.size` graph signal
        // see one consistent value.
        runner.set_base_size(base_size);

        // The document's DPI is stroke-constant too. Seeded here beside the
        // other two so a stroke engine that forgot to publish it is
        // unrepresentable, and before the diameters below, which convert
        // through the factor it implies.
        runner.set_dpi(dpi);

        // How many dabs land on one texel as the brush passes over it once.
        // A texel is inside every dab whose centre is within a radius of it,
        // so that is one diameter of travel divided by the step, at the
        // default 10% spacing, ten. Terminals accumulating a per-dab
        // quantity divide their rate by this so the knob means "per pass"
        // and stops moving when the spacing setting does. Both the diameter
        // and the spacing distance are canvas pixels, so the reference-pixel
        // dab reference crosses the boundary here.
        let diameter = base_size * DAB_REFERENCE_SIZE as f32 * runner.dpi_factor();
        let step = spacing.distance(diameter);
        runner.set_dabs_per_pass((diameter / step).max(1.0));

        let d = Self::default_diameter(runner.dpi_factor());
        Self {
            runner,
            record: StrokeRecord::new(color, "default".into()),
            spacing,
            stabilizer,
            rendered: DivergenceDiff::new(divergence_epsilon),
            stabilized_events: 0,
            save_points: SavePointStore::new(),
            last_point: None,
            accumulated_distance: 0.0,
            leftover_distance: 0.0,
            last_dab_size: [d, d],
            last_dab_pos: None,
            dab_count: 0,
            time_origin_ms: None,
            stamp_angle: None,
            stamp_angle_rate,
            stroke_seed,
            clone_source_anchor,
            clone_dest_anchor: None,
            clone_source_frame: None,
        }
    }

    /// A seed drawn from the wall clock, so two strokes of the same brush
    /// scatter differently. What a stroke the painter is making wants, and
    /// what a stroke rendered into a cached thumbnail or a documentation asset
    /// must not have, which is why it is the caller's to choose.
    /// Texel format the stroke scratch must be allocated in for this
    /// stroke's brush: see
    /// [`BrushGraphRunner::scratch_format`](crate::brush::eval::BrushGraphRunner::scratch_format).
    /// The engine builds its `StrokeEngine` before its `StrokeBuffer`, so
    /// this is available at allocation time.
    pub fn scratch_format(&self) -> wgpu::TextureFormat {
        self.runner.scratch_format()
    }

    /// How the stroke's terminal writes its scratch per dab: see
    /// [`BrushGraphRunner::dab_pass`](crate::brush::eval::BrushGraphRunner::dab_pass).
    pub fn dab_pass(&self) -> crate::brush::node::DabPass {
        self.runner.dab_pass()
    }

    pub fn random_seed() -> u32 {
        web_time::SystemTime::now()
            .duration_since(web_time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u32)
            .unwrap_or(42)
    }

    /// Set the clone source snapshot's plane-space frame for the current
    /// stroke. Called by the engine every stroke flush, before rendering;
    /// see the field doc for what the frame is.
    pub fn set_clone_source_frame(&mut self, frame: crate::coord::CanvasRect) {
        self.clone_source_frame = Some(frame);
    }

    /// Default dab diameter in canvas pixels for initial spacing (before the
    /// first dab is evaluated). `DAB_REFERENCE_SIZE` is a reference-pixel
    /// length, so the caller passes the document's reference-to-canvas
    /// factor.
    fn default_diameter(dpi_factor: f32) -> f32 {
        DAB_REFERENCE_SIZE as f32 * 0.5 * dpi_factor
    }

    /// The effective canvas-space diameter for spacing and bounding rect.
    fn effective_diameter(&self) -> f32 {
        self.last_dab_size[0].max(self.last_dab_size[1])
    }

    /// Record a raw pointer event. The stabilizer takes every event recorded
    /// since it last ran as one batch at the next [`Self::take_divergence`],
    /// however many arrive first.
    pub fn stabilize(&mut self, raw: PaintInformation) {
        self.record.push(raw);
    }

    /// Hand the stabilizer every event recorded since it last ran, as one
    /// batch.
    fn stabilize_pending(&mut self) {
        self.stabilizer
            .push_all(&self.record.events[self.stabilized_events..]);
        self.stabilized_events = self.record.events.len();
    }

    /// Whether events have been recorded since the last
    /// [`Self::take_divergence`].
    pub fn has_unrendered_input(&self) -> bool {
        self.stabilized_events < self.record.events.len()
    }

    /// Number of stabilized vertices recorded as rendered: the index of the
    /// first vertex a render that rewinds nothing starts from.
    pub fn rendered_len(&self) -> usize {
        self.rendered.len()
    }

    /// Diff the stabilized polyline against where it was last rendered, then
    /// record it as rendered: the earliest vertex to re-render, or `None`
    /// when nothing rendered has moved and only the vertices from
    /// [`Self::rendered_len`] (read before this call) need rendering. Called
    /// once per render of the stroke, whatever number of events it covers.
    pub fn take_divergence(&mut self) -> Option<usize> {
        self.stabilize_pending();
        let window = self.stabilizer.max_divergence_window();
        self.rendered.update(self.stabilizer.stabilized(), window)
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
        let d = Self::default_diameter(self.runner.dpi_factor());
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
        // `self.stabilizer.stabilized()` borrow tightly, copying the
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

        // When resuming mid-polyline, snap last_point.pos to the current
        // stabilized position of vertex `start - 1`. The stabilizer only
        // reports a vertex as moved once it shifts by at least its divergence
        // epsilon, so that vertex can drift a little, event after event,
        // after the checkpoint holding `last_point` was captured. Without the
        // snap the first re-rendered segment starts off its vertex and the
        // dab chain shows a step.
        if start > 0 {
            let snap_pos = self.stabilizer.stabilized().get(start - 1).map(|p| p.pos);
            if let (Some(pos), Some(lp)) = (snap_pos, self.last_point.as_mut()) {
                lp.pos = pos;
            }
        }

        // Walk the polyline, computing derived values and placing dabs.
        for i in start..=end {
            let mut info = self.stabilizer.stabilized()[i];

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

            // Dabs lie on the straight segment from the previous vertex to
            // this one, so a segment is final as soon as both endpoints
            // exist: nothing about it depends on vertices yet to arrive.
            let seg_len = (info.pos[0] - prev.pos[0]).hypot(info.pos[1] - prev.pos[1]);
            info.derive_sensors(Some(&prev), seg_len);
            self.accumulated_distance = info.distance;

            if seg_len < 0.001 {
                self.last_point = Some(info);
                self.save_points
                    .finalize_render_state(i, self.capture_render_state());
                continue;
            }

            let mut traveled = self.leftover_distance;
            while traveled < seg_len {
                let dab_info = lerp_paint_info(&prev, &info, traveled / seg_len);
                self.place_dab(&dab_info, gpu, i);
                let step = self.spacing.distance(self.effective_diameter());
                debug_assert!(
                    step >= super::spacing::ABSOLUTE_MIN_SPACING_PX,
                    "dab spacing dropped below 1px: {step}"
                );
                traveled += step;
            }

            self.leftover_distance = traveled - seg_len;
            self.last_point = Some(info);

            // Capture end-of-segment state on ALL save points for this vector
            // index.  This represents "everything through vector index i is
            // fully processed"; the checkpoint restore starts from i+1.
            self.save_points
                .finalize_render_state(i, self.capture_render_state());
        }

        // Phase-end flush for dab-batching terminals (paint, watercolor_batched):
        // dispatch the batched dab queue before this phase's submit_final.
        // Fragment-path terminals no-op here.
        self.runner.flush_dabs(gpu);
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
        // Motion is a per-dab quantity: the previous-dab → this-dab delta.
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
            // The engine refreshes the frame every stroke flush before any
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
        // Queue depth before the terminal runs: a dab that lands in the
        // queue but publishes no footprint is a programming error, caught
        // by the debug-assert below.
        let queued_before = gpu.dab_batch.count;
        self.runner.execute_gpu(gpu);

        gpu.flush_if_needed();

        // Update `last_dab_size` from whichever terminal in the graph
        // publishes a `dab_size` output. The runner cached the slot at
        // build time, so a new terminal that publishes the same port is
        // picked up automatically: no hand-written terminal-name list
        // to keep in sync.
        if let Some(size) = self.runner.last_dab_size() {
            self.last_dab_size = size;
        }

        // Dab bounding box for save points, in canvas coords: the footprint
        // the terminal published for the pass it issued (post-scatter,
        // post-anything else the graph did). A dab that wrote nothing (zero
        // diameter, entirely off-extent, an identity-transform early-out)
        // publishes nothing and records an empty rect, which unions away.
        //
        // There is deliberately no geometric fallback here. An envelope
        // derived from `pos ± radius` omits the compiled brush's extent
        // inflation, so it can bound the checkpoint more tightly than the
        // shader writes, and a rewind then clears pixels it cannot restore.
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

        // A dab-batching terminal holds its whole queue until the phase
        // flushes, in one buffer sized to `MAX_DABS_PER_PHASE`. A phase that
        // reaches the cap (a long path drawn in one go) flushes and submits
        // here so the next dab starts a fresh queue.
        if gpu.dab_batch.count >= MAX_DABS_PER_PHASE {
            self.runner.flush_dabs(gpu);
            gpu.submit_and_continue("brush-dab-cap-flush");
        }
    }

    /// Delegate the stroke-start / rewind-boundary lifecycle hook to every
    /// GPU terminal in the graph. Called by the engine at the start of a
    /// stroke and at every rewind boundary (full or partial): the paint
    /// terminal clears its scratch here; other terminals (warp, smudge, …)
    /// may copy the pre-stroke layer, etc. `region` is `None` for the whole
    /// scratch and `Some(rect)` (write-side local) for the part a partial
    /// rewind undoes.
    pub fn begin_stroke(
        &mut self,
        gpu: &mut BrushGpuContext,
        region: Option<crate::coord::LayerRect>,
    ) {
        self.runner.begin_stroke(gpu, region);
    }

    /// Render the whole stabilized polyline from the terminal's stroke-start
    /// state and commit it, all into `gpu`: the stroke as a from-scratch
    /// re-render of its final polyline would leave it. For a path whose
    /// every sample is known up front, such as the brush preview, so no
    /// segment is ever drawn without its real lookahead.
    pub fn render_whole(&mut self, gpu: &mut BrushGpuContext) {
        self.stabilize_pending();
        self.begin_stroke(gpu, None);
        self.reset_render_state();
        if let Some(end) = self.stabilizer.len().checked_sub(1) {
            self.render_from_stabilized_range_to(gpu, 0, end);
        }
        self.commit(gpu);
    }

    /// Delegate the per-flush commit hook to every GPU terminal. Called once
    /// per stroke flush after the flush's dabs have rendered into the
    /// scratch.
    pub fn commit(&mut self, gpu: &mut BrushGpuContext) {
        self.runner.commit(gpu);
    }

    /// Finish the stroke, consuming the engine and returning the record.
    pub fn end(self) -> StrokeRecord {
        self.record
    }

    /// Seconds between the stroke's first event and a pointer timestamp in
    /// ms. The first call fixes the origin.
    pub fn stroke_seconds(&mut self, time_ms: f64) -> f32 {
        let origin = *self.time_origin_ms.get_or_insert(time_ms);
        ((time_ms - origin) / 1000.0) as f32
    }

    /// Number of dabs placed so far.
    pub fn dab_count(&self) -> u32 {
        self.dab_count
    }
}

/// Per-dab motion: delta from the previous emitted dab. `tracker` is the
/// position of the most recently emitted dab, or `None` at stroke start /
/// after a rewind. Returns `[0, 0]` when there is no previous dab: that's
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
/// diameter, and `rate` the permitted turn in radians per diameter of travel,
/// or [`STAMP_ANGLE_RATE_UNLIMITED`], at which the cap is skipped entirely and
/// only the fold applies.
///
/// The axis fold (taking whichever of `direction` / `direction + π` is nearer
/// to the held orientation) is unconditional. A symmetric stamp is identical
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
    /// would seed `motion=[100,0]` for every dab: wrong for smudge. After
    /// the fix, each dab sees its own ~5px step.
    #[test]
    fn motion_is_per_dab_delta_not_segment_delta() {
        let mut tracker: Option<[f32; 2]> = None;

        // First dab: no prior dab, motion must be zero.
        assert_eq!(advance_dab_motion(&mut tracker, [0.0, 0.0]), [0.0, 0.0]);

        // 20 dabs at 5px spacing along x: each motion must be ~5px, not 100px.
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

    /// Zero travel permits zero rotation whenever the cap is engaged: a
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
    /// shipped default, so this guards the promise that a brush which never
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
    /// leaves its `drawing_angle` at the default 0; see
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
    /// representative instead, identical for a symmetric stamp, and the
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

    // ── DivergenceDiff ──────────────────────────────────────────────────

    fn at(x: f32) -> PaintInformation {
        PaintInformation {
            pos: [x, 0.0],
            ..Default::default()
        }
    }

    /// Regression: a vertex that moved is reported even when a vertex nearer
    /// the tip did not. The walk used to stop at the first unchanged vertex,
    /// so vertices behind a freshly rendered one accumulated drift unchecked
    /// and snapped into place much later, leaving a visible disconnect.
    #[test]
    fn moved_vertex_behind_an_unchanged_one_is_reported() {
        let mut diff = DivergenceDiff::new(0.5);
        assert_eq!(diff.update(&[at(0.0), at(10.0), at(20.0)], 10), None);
        assert_eq!(
            diff.update(&[at(0.0), at(10.6), at(20.0)], 10),
            Some(1),
            "the moved vertex sits behind an unchanged tip"
        );
    }

    /// Regression: the divergence walk starts from the tip as last rendered,
    /// not from the current tip. When several pushes land between two
    /// renders, a push's reach is measured from the tip that existed before
    /// it, and the earliest of those is the rendered one; walking from the
    /// current tip skipped every moved vertex the frame's commits had pushed
    /// out of the window.
    #[test]
    fn divergence_walk_starts_from_the_rendered_tip() {
        let mut diff = DivergenceDiff::new(DIVERGENCE_EPSILON);
        let rendered: Vec<_> = (0..10).map(|i| at(i as f32 * 6.0)).collect();
        assert_eq!(diff.update(&rendered, 5), None);
        let mut current: Vec<_> = (0..30).map(|i| at(i as f32 * 6.0)).collect();
        current[8].pos[1] = 1.0;
        assert_eq!(
            diff.update(&current, 5),
            Some(8),
            "vertex 8 moved, within the window behind the rendered tip at 9"
        );
    }
}
