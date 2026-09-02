//! Stroke stabilizer: retroactive stroke reshaping with zero lag.
//!
//! The stabilizer processes the full stroke history before dabs are placed.
//! It operates outside the per-dab node graph: brushes configure which
//! algorithm to use and its parameters, and the engine constructs the
//! algorithm at stroke start.
//!
//! Follows the same modular registry pattern as veils (`gpu/veil.rs` +
//! `gpu/veils/*.rs`): each algorithm is a self-contained module that
//! declares its own params and factory.  A registry maps type_id →
//! registration.  New algorithms are added by dropping a `.rs` file in
//! `brush/stabilizers/`: no other files touched.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paint_info::PaintInformation;
use crate::gpu::params::{ParamDef, ParamValue};

/// Result of pushing a new point through the stabilizer.
pub struct StabilizeResult {
    /// Earliest dab index that needs re-rendering (everything from here
    /// to the tip has changed).  `None` means nothing diverged: only
    /// new points were appended.
    pub divergence_index: Option<usize>,
}

/// Threshold in pixels below which a stabilized point is considered
/// unchanged between frames.
const DIVERGENCE_EPSILON: f32 = 0.5;

/// Find the earliest index whose position changed from the previous frame,
/// walking backward from the tip until either the per-index delta falls
/// below [`DIVERGENCE_EPSILON`] or the influence bound `max_window` is hit.
///
/// `current` is this frame's stabilized polyline, `prev_positions` the
/// previous frame's positions. The walk is bounded to `max_window` indices
/// behind the tip (the caller's model of how far a perturbation can reach),
/// so it never reports divergence at indices the model says cannot have moved.
///
/// Shared by [`LaplacianStabilizer`](crate::brush::stabilizers::laplacian::LaplacianStabilizer)
/// (over its real polyline) and [`PredictingStabilizer`] (over its combined
/// real+predicted polyline): one detector, two callers.
pub fn find_divergence(
    current: &[PaintInformation],
    prev_positions: &[[f32; 2]],
    max_window: usize,
) -> Option<usize> {
    let len = current.len();
    if len == 0 {
        return None;
    }

    // `earliest` is the lowest index whose position could possibly differ
    // from the previous frame given the influence bound. Walking past it is
    // wasted work and risks reporting spurious divergence.
    let earliest = len.saturating_sub(max_window + 1);
    let eps2 = DIVERGENCE_EPSILON * DIVERGENCE_EPSILON;

    // Squared position delta at index `i` (within bounds of both arrays here).
    let delta2 = |i: usize| -> f32 {
        let cur = current[i].pos;
        let prev = prev_positions[i];
        let dx = cur[0] - prev[0];
        let dy = cur[1] - prev[1];
        dx * dx + dy * dy
    };

    if prev_positions.len() == len {
        // Same length: walk backward from tip to `earliest`.
        for i in (earliest..len).rev() {
            if delta2(i) < eps2 {
                return if i + 1 < len { Some(i + 1) } else { None };
            }
        }
        Some(earliest)
    } else {
        // Polyline grew. New indices `[prev_positions.len(), len-1]` are by
        // definition new and cannot be compared. Existing indices that
        // overlap with `prev_positions` are in `[0, overlap_end)`: walk
        // those, descending, bounded below by `earliest`.
        let overlap_end = prev_positions.len().min(len);
        if earliest >= overlap_end {
            // No overlap to check (e.g., first push of a stroke). The
            // divergence index is `earliest` itself, which equals 0 when
            // there is nothing prior to compare against.
            return Some(earliest);
        }
        for i in (earliest..overlap_end).rev() {
            if delta2(i) < eps2 {
                return Some(i + 1);
            }
        }
        Some(earliest)
    }
}

/// The trait that all stabilizer algorithms implement.
pub trait StabilizerAlgorithm: Send {
    /// Append a raw input point, run the algorithm, and return the result.
    fn push(&mut self, point: PaintInformation) -> StabilizeResult;

    /// The current stabilized polyline (full stroke).
    fn stabilized(&self) -> &[PaintInformation];

    /// Number of points in the stabilized polyline.
    fn len(&self) -> usize {
        self.stabilized().len()
    }

    /// Whether the stabilized polyline is empty.
    fn is_empty(&self) -> bool {
        self.stabilized().is_empty()
    }

    /// Conservative upper bound on how far back from the tip divergence
    /// can reach (in vector indices). Used to space checkpoints so the
    /// oldest one is past the divergence boundary.
    fn max_divergence_window(&self) -> usize {
        0
    }

    /// Reset for a new stroke.
    fn clear(&mut self);
}

/// A pass-through "stabilizer" that does nothing: output equals input.
/// Used when no stabilization is configured (empty algorithm string).
pub struct PassThrough {
    points: Vec<PaintInformation>,
}

impl Default for PassThrough {
    fn default() -> Self {
        Self::new()
    }
}

impl PassThrough {
    pub fn new() -> Self {
        Self {
            points: Vec::with_capacity(256),
        }
    }
}

impl StabilizerAlgorithm for PassThrough {
    fn push(&mut self, point: PaintInformation) -> StabilizeResult {
        self.points.push(point);
        StabilizeResult {
            divergence_index: None,
        }
    }

    fn stabilized(&self) -> &[PaintInformation] {
        &self.points
    }

    fn clear(&mut self) {
        self.points.clear();
    }
}

/// Minimum real samples before prediction engages: enough for a stable
/// heading and a measured inter-sample Δt. Below this the decorator is a
/// pass-through of the inner stabilizer's result.
const MIN_REAL_FOR_PREDICTION: usize = 3;

/// Hard cap on the predicted point count, so a pathologically high sample
/// rate (tiny Δt) can't blow up the divergence window / checkpoint spacing.
const MAX_PREDICTED_POINTS: usize = 32;

/// Number of recent real segments the heading, per-sample step, and Δt are
/// averaged over: smooths raw last-two-frame jitter.
const HEADING_WINDOW: usize = 3;

/// Prediction decorator: wraps a real stabilizer and appends a short
/// extrapolated tail past the real tip, so ink appears ahead of the pen and
/// hides the residual pen-to-pixel latency.
///
/// The predicted points live in `stabilized()` **and** in the buffer
/// [`find_divergence`] diffs, so the engine's existing rewind rewrites them
/// every frame: no separate render target, no parallel path. The predicted
/// count is held constant once established, so the combined polyline only ever
/// grows-by-one + reshapes: the cases `find_divergence` already handles.
///
/// Only constructed when a real stabilizer is active (strength > 0) and a
/// look-ahead horizon is configured (> 0); see the engine's stroke-start path
/// and `docs/plans/stroke-prediction-stabilizer.md`.
pub struct PredictingStabilizer {
    inner: Box<dyn StabilizerAlgorithm>,
    /// Real + predicted polyline: what `stabilized()` returns.
    combined: Vec<PaintInformation>,
    /// `combined` positions from the previous push (divergence diff input).
    prev_positions: Vec<[f32; 2]>,
    /// Look-ahead horizon in seconds (converted from the ms port value).
    horizon_secs: f32,
    /// Predicted point count, fixed once prediction engages. `None` while
    /// ramping up (too few real samples for a stable heading / Δt).
    held_count: Option<usize>,
}

impl PredictingStabilizer {
    /// Wrap `inner` with prediction over a `horizon_ms` millisecond look-ahead.
    pub fn new(inner: Box<dyn StabilizerAlgorithm>, horizon_ms: f32) -> Self {
        Self {
            inner,
            combined: Vec::with_capacity(256),
            prev_positions: Vec::with_capacity(256),
            horizon_secs: (horizon_ms / 1000.0).max(0.0),
            held_count: None,
        }
    }

    /// The held predicted-point count. Establishes it once, on the first
    /// frame with enough real samples and a measurable cadence, from the
    /// horizon and the recent mean Δt; returns 0 while still ramping.
    ///
    /// `self.combined[..real_len]` holds the real polyline at call time (the
    /// predicted tail has not been appended yet).
    fn resolve_count(&mut self, real_len: usize) -> usize {
        if let Some(n) = self.held_count {
            return n;
        }
        // Horizon 0 → prediction off; the decorator is a transparent
        // pass-through of the inner result (never latched to a nonzero count).
        if self.horizon_secs <= 0.0 {
            return 0;
        }
        if real_len < MIN_REAL_FOR_PREDICTION {
            return 0;
        }
        let mean_dt = self.recent_mean_dt(real_len);
        if mean_dt <= 0.0 {
            return 0;
        }
        // N predicted samples span the configured *time* horizon at the
        // current sample cadence: a time horizon, not a fixed dab count, so
        // the predicted distance auto-scales with pen speed.
        let n = (self.horizon_secs / mean_dt).round() as usize;
        let n = n.clamp(1, MAX_PREDICTED_POINTS);
        self.held_count = Some(n);
        n
    }

    /// Mean inter-sample Δt over the last `HEADING_WINDOW` real segments.
    fn recent_mean_dt(&self, real_len: usize) -> f32 {
        let k = HEADING_WINDOW.min(real_len - 1);
        if k == 0 {
            return 0.0;
        }
        let dt = self.combined[real_len - 1].time - self.combined[real_len - 1 - k].time;
        dt / k as f32
    }

    /// Append `n` extrapolated points past the real tip. Reads the real
    /// prefix `self.combined[..real_len]` by value (all `Copy`) before pushing.
    fn append_prediction(&mut self, real_len: usize, n: usize) {
        let k = HEADING_WINDOW.min(real_len - 1);
        let tip = self.combined[real_len - 1];
        let base = self.combined[real_len - 1 - k];
        let dx = tip.pos[0] - base.pos[0];
        let dy = tip.pos[1] - base.pos[1];
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1.0e-4 {
            // Stationary: no meaningful heading; collapse onto the tip.
            for _ in 0..n {
                self.combined.push(tip);
            }
            return;
        }
        let heading = [dx / dist, dy / dist];
        // Per-point step = the recent smoothed per-sample displacement. This
        // makes the predicted distance speed-proportional by construction,
        // whatever the sample rate.
        let step = dist / k as f32;

        // Curvature/reversal damping pulls the tail toward the tip rather
        // than removing points: kills the reversal "whisker" and keeps the
        // point count constant.
        let damp = self.reversal_damp(real_len, heading, k);

        for j in 1..=n {
            let d = step * j as f32 * damp;
            let mut p = tip;
            p.pos = [tip.pos[0] + heading[0] * d, tip.pos[1] + heading[1] * d];
            self.combined.push(p);
        }
    }

    /// Damping factor in [0, 1] from heading alignment: 1 when the stroke
    /// continues straight, 0 when it reverses onto itself.
    fn reversal_damp(&self, real_len: usize, heading: [f32; 2], k: usize) -> f32 {
        // Need a preceding segment of the same span to compare against.
        if real_len < 2 * k + 1 {
            return 1.0;
        }
        let a = self.combined[real_len - 1 - k];
        let b = self.combined[real_len - 1 - 2 * k];
        let dx = a.pos[0] - b.pos[0];
        let dy = a.pos[1] - b.pos[1];
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1.0e-4 {
            return 1.0;
        }
        let prev_heading = [dx / dist, dy / dist];
        let align = heading[0] * prev_heading[0] + heading[1] * prev_heading[1];
        align.clamp(0.0, 1.0)
    }
}

impl StabilizerAlgorithm for PredictingStabilizer {
    fn push(&mut self, point: PaintInformation) -> StabilizeResult {
        // Save previous combined positions for the divergence diff.
        self.prev_positions.clear();
        self.prev_positions
            .extend(self.combined.iter().map(|p| p.pos));

        // Advance the inner (real) stabilizer, then rebuild the combined
        // polyline from its relaxed output.
        self.inner.push(point);
        self.combined.clear();
        self.combined.extend_from_slice(self.inner.stabilized());
        let real_len = self.combined.len();

        // Append the predicted extension (held count; 0 while ramping).
        let n = self.resolve_count(real_len);
        if n > 0 {
            self.append_prediction(real_len, n);
        }

        // Divergence over the FULL combined polyline (not the inner's
        // real-only result), with the widened window, which is what makes the
        // existing rewind rewrite the predicted tail every frame.
        let divergence_index = find_divergence(
            &self.combined,
            &self.prev_positions,
            self.max_divergence_window(),
        );
        StabilizeResult { divergence_index }
    }

    fn stabilized(&self) -> &[PaintInformation] {
        &self.combined
    }

    /// The inner window widened by the constant predicted count, so the
    /// checkpoint ring spaces its snapshots deep enough to rewind over the
    /// predicted region and the engine's coverage assert still holds.
    fn max_divergence_window(&self) -> usize {
        self.inner.max_divergence_window() + self.held_count.unwrap_or(0)
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.combined.clear();
        self.prev_positions.clear();
        self.held_count = None;
    }
}

/// What each stabilizer module returns from its `register()` function.
pub struct StabilizerRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
    pub from_params: fn(&[ParamValue]) -> Box<dyn StabilizerAlgorithm>,
}

/// Auto-discovered stabilizer registry.
pub struct StabilizerRegistry {
    entries: HashMap<&'static str, StabilizerRegistration>,
}

impl Default for StabilizerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl StabilizerRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::stabilizers::registrations() {
            entries.insert(reg.type_id, reg);
        }
        StabilizerRegistry { entries }
    }

    /// Return all registered stabilizer type IDs with their parameter definitions.
    pub fn types(&self) -> Vec<(&'static str, &'static str, &'static [ParamDef])> {
        let mut types: Vec<_> = self
            .entries
            .iter()
            .map(|(&id, reg)| (id, reg.display_name, reg.params))
            .collect();
        types.sort_by_key(|(id, _, _)| *id);
        types
    }

    /// Get the static parameter definitions for a stabilizer type.
    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries.get(type_id).map(|e| e.params).unwrap_or(&[])
    }

    /// Create a stabilizer algorithm instance from a type string and parameters.
    /// Returns `None` if the type_id is not found.
    pub fn create(
        &self,
        type_id: &str,
        params: &[ParamValue],
    ) -> Option<Box<dyn StabilizerAlgorithm>> {
        self.entries
            .get(type_id)
            .map(|reg| (reg.from_params)(params))
    }

    /// Create a stabilizer from a `StabilizerConfig`.
    /// Returns a pass-through if the config has no algorithm set.
    pub fn create_from_config(&self, config: &StabilizerConfig) -> Box<dyn StabilizerAlgorithm> {
        if config.algorithm.is_empty() || config.algorithm == "none" {
            return Box::new(PassThrough::new());
        }
        self.create(&config.algorithm, &config.params)
            .unwrap_or_else(|| {
                log::warn!(
                    "unknown stabilizer algorithm '{}', using pass-through",
                    config.algorithm
                );
                Box::new(PassThrough::new())
            })
    }
}

/// Per-brush stabilizer configuration: stored in `BrushMetadata`.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct StabilizerConfig {
    /// Algorithm type_id.  Empty string or "none" = pass-through.
    #[serde(default)]
    pub algorithm: String,
    /// Algorithm-specific parameter values.
    #[serde(default)]
    pub params: Vec<ParamValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_through_identity() {
        let mut stab = PassThrough::new();
        for i in 0..5 {
            let pt = PaintInformation {
                pos: [i as f32 * 10.0, 0.0],
                pressure: 0.5,
                ..Default::default()
            };
            let result = stab.push(pt);
            assert!(result.divergence_index.is_none());
        }
        assert_eq!(stab.len(), 5);
        // Points are unchanged (no smoothing).
        assert!((stab.stabilized()[2].pos[0] - 20.0).abs() < 1e-6);
    }

    #[test]
    fn pass_through_clear() {
        let mut stab = PassThrough::new();
        stab.push(PaintInformation::default());
        assert_eq!(stab.len(), 1);
        stab.clear();
        assert_eq!(stab.len(), 0);
    }

    #[test]
    fn stabilizer_config_default_is_pass_through() {
        let config = StabilizerConfig::default();
        assert!(config.algorithm.is_empty());
        assert!(config.params.is_empty());
    }

    #[test]
    fn stabilizer_config_serde_round_trip() {
        let config = StabilizerConfig {
            algorithm: "laplacian".into(),
            params: vec![ParamValue::Float(0.6)],
        };
        let json = serde_json::to_string(&config).unwrap();
        let loaded: StabilizerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.algorithm, "laplacian");
        assert_eq!(loaded.params.len(), 1);
    }

    #[test]
    fn stabilizer_config_missing_fields_default() {
        let json = "{}";
        let config: StabilizerConfig = serde_json::from_str(json).unwrap();
        assert!(config.algorithm.is_empty());
        assert!(config.params.is_empty());
    }

    #[test]
    fn registry_creates_from_config() {
        let registry = StabilizerRegistry::new();

        // Empty config → pass-through.
        let config = StabilizerConfig::default();
        let stab = registry.create_from_config(&config);
        assert_eq!(stab.len(), 0);

        // "none" → pass-through.
        let config = StabilizerConfig {
            algorithm: "none".into(),
            params: vec![],
        };
        let stab = registry.create_from_config(&config);
        assert_eq!(stab.len(), 0);

        // Known algorithm.
        let config = StabilizerConfig {
            algorithm: "laplacian".into(),
            params: vec![ParamValue::Float(0.5)],
        };
        let mut stab = registry.create_from_config(&config);
        stab.push(PaintInformation::default());
        assert_eq!(stab.len(), 1);
    }

    #[test]
    fn registry_discovers_algorithms() {
        let registry = StabilizerRegistry::new();
        let types = registry.types();
        assert!(
            !types.is_empty(),
            "registry should discover at least one algorithm"
        );
        assert!(types.iter().any(|(id, _, _)| *id == "laplacian"));
    }

    // ── PredictingStabilizer ────────────────────────────────────────────

    /// A pen sample at position `(x, y)` and timestamp `t` (seconds).
    fn mk(x: f32, y: f32, t: f32) -> PaintInformation {
        PaintInformation {
            pos: [x, y],
            pressure: 0.5,
            time: t,
            ..Default::default()
        }
    }

    /// A laplacian inner stabilizer at the given strength, via the registry
    /// (avoids depending on the generated module path).
    fn laplacian_inner(strength: f32) -> Box<dyn StabilizerAlgorithm> {
        StabilizerRegistry::new()
            .create("laplacian", &[ParamValue::Float(strength)])
            .expect("laplacian registered")
    }

    fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
        let dx = a[0] - b[0];
        let dy = a[1] - b[1];
        (dx * dx + dy * dy).sqrt()
    }

    /// T-B: with prediction on and a straight stroke, `stabilized()` extends
    /// past the last real point along its heading by ~the horizon (fixed
    /// point count).
    #[test]
    fn prediction_extends_tip_on_straight_stroke() {
        // 30ms horizon, samples 10px / 10ms apart ⇒ N = round(30/10) = 3.
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 30.0);
        for i in 0..8 {
            stab.push(mk(i as f32 * 10.0, 0.0, i as f32 * 0.01));
        }
        let n = 3;
        let pts = stab.stabilized();
        assert_eq!(pts.len(), 8 + n, "combined = 8 real + {n} predicted");

        // The last real point of a straight stroke stays pinned at x = 70.
        let real_tip_x = pts[7].pos[0];
        assert!((real_tip_x - 70.0).abs() < 1e-3);
        for pred in &pts[8..8 + n] {
            assert!(
                pred.pos[0] > real_tip_x,
                "predicted x {} should extend past the tip",
                pred.pos[0]
            );
            assert!(pred.pos[1].abs() < 1e-2, "straight stroke stays on y=0");
        }
        // Last predicted point ≈ horizon ahead: 3 steps × 10px = 30px past tip.
        assert!((pts[10].pos[0] - (real_tip_x + 30.0)).abs() < 2.0);
    }

    /// T-C: after a turn, the combined-polyline divergence lands at/inside the
    /// prediction boundary and stays within the widened window, and the
    /// predicted tail is rewritten to follow the turn (self-correction).
    #[test]
    fn prediction_self_corrects_via_combined_divergence() {
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 30.0);
        for i in 0..8 {
            stab.push(mk(i as f32 * 10.0, 0.0, i as f32 * 0.01));
        }
        // Straight-stroke prediction runs along +x (y≈0).
        assert!(stab.stabilized().last().unwrap().pos[1].abs() < 1e-2);

        // A turn downward.
        let r = stab.push(mk(80.0, 30.0, 0.08));
        let real_len = 9; // 9 real points, N = 3 predicted
        let tip_vi = stab.stabilized().len() - 1;
        let window = stab.max_divergence_window();

        let div = r.divergence_index.expect("a turn must diverge");
        assert!(
            div <= real_len,
            "divergence {div} must cover the predicted tail (<= real_len {real_len})"
        );
        assert!(
            div >= tip_vi.saturating_sub(window),
            "divergence {div} must stay within the widened window \
             (tip_vi {tip_vi}, window {window})"
        );

        // The predicted tail now heads into the turn (y grew from ~0).
        let last_pred_y = stab.stabilized().last().unwrap().pos[1];
        assert!(
            last_pred_y > 5.0,
            "predicted tail should follow the turn, got y={last_pred_y}"
        );
    }

    /// T-D: a sharp reversal collapses the predicted extension toward the real
    /// tip (no overshoot whisker) while keeping the point count constant.
    #[test]
    fn reversal_collapses_predicted_tail() {
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 30.0);
        // Rightward…
        for i in 0..6 {
            stab.push(mk(i as f32 * 10.0, 0.0, i as f32 * 0.01));
        }
        // …then reverse back leftward.
        stab.push(mk(40.0, 0.0, 0.06));
        stab.push(mk(30.0, 0.0, 0.07));
        stab.push(mk(20.0, 0.0, 0.08));

        let n = 3;
        let pts = stab.stabilized();
        let real_len = pts.len() - n;
        assert_eq!(pts.len(), real_len + n, "point count stays constant at N");

        let tip = pts[real_len - 1].pos;
        // A straight extension would place the far predicted point ~N×step
        // (≈30px) away; damping pulls it in to well under one step.
        let far = dist(pts[pts.len() - 1].pos, tip);
        assert!(
            far < 10.0,
            "reversed predicted tail should collapse toward the tip, got {far}px"
        );
    }

    /// T-E: horizon 0 ⇒ the decorator is transparent: `stabilized()` and the
    /// divergence result match a bare inner stabilizer, frame for frame.
    #[test]
    fn horizon_zero_is_transparent() {
        let mut pred = PredictingStabilizer::new(laplacian_inner(0.5), 0.0);
        let mut bare = laplacian_inner(0.5);
        for i in 0..8 {
            let p = mk(i as f32 * 10.0, (i as f32).sin() * 5.0, i as f32 * 0.01);
            let rp = pred.push(p);
            let rb = bare.push(p);
            assert_eq!(rp.divergence_index, rb.divergence_index, "step {i}");
        }
        assert_eq!(pred.max_divergence_window(), bare.max_divergence_window());
        assert_eq!(pred.stabilized().len(), bare.stabilized().len());
        for (a, b) in pred.stabilized().iter().zip(bare.stabilized()) {
            assert!(dist(a.pos, b.pos) < 1e-6, "positions must match bare inner");
        }
    }

    /// T-H: with several real samples per frame (a high-Hz burst), the
    /// per-point predicted step tracks the measured per-sample displacement,
    /// not a fixed per-frame step: the guard against "one sample = one frame".
    #[test]
    fn predicted_step_tracks_per_sample_displacement() {
        // 240Hz-ish burst: 5px apart, ~4-5ms apart (unequal Δt), 40ms horizon.
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 40.0);
        let times = [0.0, 0.004, 0.009, 0.013, 0.018, 0.022, 0.027];
        for (i, &t) in times.iter().enumerate() {
            stab.push(mk(i as f32 * 5.0, 0.0, t));
        }
        // Predicted points are the ones past the last real point (x = 30).
        let preds: Vec<_> = stab
            .stabilized()
            .iter()
            .filter(|p| p.pos[0] > 30.0 + 1e-3)
            .collect();
        assert!(preds.len() >= 2, "expected a multi-point predicted tail");
        let spacing = preds[1].pos[0] - preds[0].pos[0];
        assert!(
            (spacing - 5.0).abs() < 1.5,
            "predicted step {spacing} should track the ~5px per-sample \
             displacement, not a per-frame step"
        );
    }

    /// T-I: at stroke start the first < 3 real samples emit no prediction;
    /// once engaged, the polyline grows by exactly one per push and the
    /// divergence never reports outside the (ramping) window.
    #[test]
    fn stroke_start_ramps_without_breaking_growth() {
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 30.0);
        let mut prev_len = 0usize;
        for i in 0..12 {
            let r = stab.push(mk(i as f32 * 10.0, 0.0, i as f32 * 0.01));
            let len = stab.stabilized().len();
            let tip_vi = len.saturating_sub(1);
            let window = stab.max_divergence_window();

            if let Some(div) = r.divergence_index {
                assert!(
                    div >= tip_vi.saturating_sub(window),
                    "step {i}: div {div} outside window (tip_vi {tip_vi}, window {window})"
                );
            }

            // Below MIN_REAL_FOR_PREDICTION real samples: no predicted tail.
            if i < MIN_REAL_FOR_PREDICTION - 1 {
                assert_eq!(len, i + 1, "step {i}: no prediction before ramp");
            } else if i > MIN_REAL_FOR_PREDICTION - 1 {
                // Past the one-time engage jump, growth is exactly one/push.
                assert_eq!(len, prev_len + 1, "step {i}: should grow by one");
            }
            prev_len = len;
        }
    }
}
