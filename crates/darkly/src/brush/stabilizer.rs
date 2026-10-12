//! Stroke stabilizer: retroactive stroke reshaping with zero lag.
//!
//! The stabilizer processes the full stroke history before dabs are placed.
//! It operates outside the per-dab node graph: brushes configure which
//! algorithm to use and its parameters, and the engine constructs the
//! algorithm at stroke start.
//!
//! Follows the same modular registry pattern as veils (`gpu/veil.rs` +
//! `gpu/effects/*.rs`): each algorithm is a self-contained module that
//! declares its own params and factory.  A registry maps type_id →
//! registration.  New algorithms are added by dropping a `.rs` file in
//! `brush/stabilizers/`: no other files touched.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paint_info::PaintInformation;
use super::resampler::{ResamplingStabilizer, RESAMPLE_SPACING_CSS_PX};
use crate::gpu::params::{ParamDef, ParamValue};

/// The trait that all stabilizer algorithms implement.
///
/// An algorithm is pure geometry: it turns input points into a polyline and
/// bounds how far a push can reach behind the tip. Which vertices moved since
/// the stroke was last rendered is the renderer's question, answered by the
/// stroke engine's [`DivergenceDiff`](super::stroke_engine::DivergenceDiff)
/// against that bound.
pub trait StabilizerAlgorithm: Send {
    /// Append `points` and run the algorithm once. The polyline afterwards is
    /// bit-identical to the one pushing each point in turn leaves. An empty
    /// slice changes nothing.
    fn push_all(&mut self, points: &[PaintInformation]);

    /// Append one raw input point and run the algorithm.
    fn push(&mut self, point: PaintInformation) {
        self.push_all(std::slice::from_ref(&point));
    }

    /// Forget the most recent point, so the next push replaces it. The
    /// polyline is only meaningful again after that push.
    fn retract_tip(&mut self);

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

    /// Conservative upper bound, in vector indices, on how far behind the tip
    /// as it stood before a push that push can move a vertex. Measured from
    /// the earlier tip, the bound holds over any number of pushes between two
    /// renders. Used to bound the divergence walk and to space checkpoints so
    /// the oldest one is past the divergence boundary.
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
    fn push_all(&mut self, points: &[PaintInformation]) {
        self.points.extend_from_slice(points);
    }

    fn retract_tip(&mut self) {
        self.points.pop();
    }

    fn stabilized(&self) -> &[PaintInformation] {
        &self.points
    }

    fn clear(&mut self) {
        self.points.clear();
    }
}

/// Minimum real vertices before prediction engages: enough for a stable
/// heading and a measured speed. Below this the decorator is a pass-through
/// of the inner stabilizer's result.
const MIN_REAL_FOR_PREDICTION: usize = 3;

/// Predicted points appended past the real tip. Constant, so the divergence
/// window is static and the tail's density does not depend on input cadence.
const PREDICTED_POINTS: usize = 3;

/// Number of recent real segments the heading and speed are measured over.
/// Under resampling the last one is the partial segment to the pinned tip;
/// a speed (distance over elapsed time) is unaffected by its shorter length.
const HEADING_WINDOW: usize = 3;

/// Prediction decorator: wraps a real stabilizer and appends a short
/// extrapolated tail past the real tip, so ink appears ahead of the pen and
/// hides the residual pen-to-pixel latency.
///
/// The predicted points live in `stabilized()`, which the stroke engine diffs
/// against what it rendered, so its existing rewind rewrites them every
/// frame: no separate render target, no parallel path. The predicted count is
/// constant once engaged, so the combined polyline grows as the inner's does
/// (by any number of vertices per push under resampling) and reshapes: both
/// cases the diff handles.
///
/// Only constructed when a real stabilizer is active (strength > 0) and a
/// look-ahead horizon is configured (> 0); see the engine's stroke-start path.
pub struct PredictingStabilizer {
    inner: Box<dyn StabilizerAlgorithm>,
    /// Real + predicted polyline: what `stabilized()` returns.
    combined: Vec<PaintInformation>,
    /// Length of the real (inner) prefix of `combined`.
    real_len: usize,
    /// Look-ahead horizon in seconds (converted from the ms port value).
    horizon_secs: f32,
    /// Vertices copied from the inner polyline, for tests that bound it.
    #[cfg(test)]
    copied_vertices: u64,
}

impl PredictingStabilizer {
    /// Wrap `inner` with prediction over a `horizon_ms` millisecond
    /// look-ahead.
    pub fn new(inner: Box<dyn StabilizerAlgorithm>, horizon_ms: f32) -> Self {
        Self {
            inner,
            combined: Vec::with_capacity(256),
            real_len: 0,
            horizon_secs: (horizon_ms / 1000.0).max(0.0),
            #[cfg(test)]
            copied_vertices: 0,
        }
    }

    /// Predicted points this decorator appends once engaged: none when the
    /// horizon is off.
    fn predicted_points(&self) -> usize {
        if self.horizon_secs > 0.0 {
            PREDICTED_POINTS
        } else {
            0
        }
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
        // The tail spans the horizon at the recent pen speed, so the
        // predicted distance is speed-proportional whatever the vertex
        // spacing or input cadence. Without a measurable elapsed time, fall
        // back to the mean per-vertex displacement.
        let dt = tip.time - base.time;
        let step = if dt > 0.0 {
            self.horizon_secs * (dist / dt) / n as f32
        } else {
            dist / k as f32
        };

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
    fn push_all(&mut self, points: &[PaintInformation]) {
        if points.is_empty() {
            return;
        }
        // Advance the inner (real) stabilizer, then rebuild the combined
        // polyline from its relaxed output. The batch cannot have moved
        // inner vertices further than its window behind the previous real
        // tip, so the prefix before that is already in `combined` and only
        // the rest, with the stale predicted tail, is replaced.
        self.inner.push_all(points);
        let inner = self.inner.stabilized();
        let unchanged = self
            .real_len
            .min(inner.len())
            .saturating_sub(self.inner.max_divergence_window() + 1);
        self.combined.truncate(unchanged);
        self.combined.extend_from_slice(&inner[unchanged..]);
        #[cfg(test)]
        {
            self.copied_vertices += (inner.len() - unchanged) as u64;
        }
        let real_len = inner.len();
        self.real_len = real_len;

        // Append the predicted extension once enough real vertices exist.
        let n = self.predicted_points();
        if n > 0 && real_len >= MIN_REAL_FOR_PREDICTION {
            self.append_prediction(real_len, n);
        }
    }

    fn retract_tip(&mut self) {
        self.inner.retract_tip();
    }

    fn stabilized(&self) -> &[PaintInformation] {
        &self.combined
    }

    /// The inner window widened by the constant predicted count, so the
    /// checkpoint ring spaces its snapshots deep enough to rewind over the
    /// predicted region and the engine's coverage assert still holds.
    fn max_divergence_window(&self) -> usize {
        self.inner.max_divergence_window() + self.predicted_points()
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.combined.clear();
        self.real_len = 0;
    }
}

/// What each stabilizer module returns from its `register()` function.
pub struct StabilizerRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
    /// Build the algorithm from its parameter values and the stroke's view
    /// scale (canvas pixels per CSS pixel), so distances and speeds the
    /// algorithm reasons about in CSS pixels can be expressed in canvas
    /// units.
    pub from_params: fn(&[ParamValue], f32) -> Box<dyn StabilizerAlgorithm>,
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

    /// Create a stabilizer algorithm instance from a type string, parameters
    /// and the stroke's view scale (canvas pixels per CSS pixel).
    /// Returns `None` if the type_id is not found.
    pub fn create(
        &self,
        type_id: &str,
        params: &[ParamValue],
        canvas_per_css_px: f32,
    ) -> Option<Box<dyn StabilizerAlgorithm>> {
        self.entries
            .get(type_id)
            .map(|reg| (reg.from_params)(params, canvas_per_css_px))
    }

    /// Create a stabilizer from a `StabilizerConfig`.
    /// Returns a pass-through if the config has no algorithm set.
    pub fn create_from_config(
        &self,
        config: &StabilizerConfig,
        canvas_per_css_px: f32,
    ) -> Box<dyn StabilizerAlgorithm> {
        if config.algorithm.is_empty() || config.algorithm == "none" {
            return Box::new(PassThrough::new());
        }
        self.create(&config.algorithm, &config.params, canvas_per_css_px)
            .unwrap_or_else(|| {
                log::warn!(
                    "unknown stabilizer algorithm '{}', using pass-through",
                    config.algorithm
                );
                Box::new(PassThrough::new())
            })
    }
}

/// Build the stabilizer stack a stroke runs through: the configured
/// algorithm fed by a [`ResamplingStabilizer`], wrapped in prediction when a
/// look-ahead horizon is set.
///
/// `canvas_per_css_px` is the canvas pixels one CSS pixel of pen travel spans
/// at the stroke's view (`device_pixel_ratio / zoom`). The resample spacing
/// and the algorithm's own CSS-pixel quantities are scaled by it, so the
/// smoothing reach is the same on screen at every zoom and pixel ratio.
/// Decorators engage only over an algorithm that can reshape rendered
/// vertices (`max_divergence_window() > 0`).
pub fn stroke_stabilizer_stack(
    registry: &StabilizerRegistry,
    config: &StabilizerConfig,
    prediction_horizon_ms: f32,
    canvas_per_css_px: f32,
) -> Box<dyn StabilizerAlgorithm> {
    let inner = registry.create_from_config(config, canvas_per_css_px);
    if inner.max_divergence_window() == 0 {
        return inner;
    }
    let inner = Box::new(ResamplingStabilizer::new(
        inner,
        RESAMPLE_SPACING_CSS_PX * canvas_per_css_px,
    ));
    if prediction_horizon_ms > 0.0 {
        Box::new(PredictingStabilizer::new(inner, prediction_horizon_ms))
    } else {
        inner
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
    use crate::brush::stroke_engine::{DivergenceDiff, DIVERGENCE_EPSILON};

    #[test]
    fn pass_through_identity() {
        let mut stab = PassThrough::new();
        for i in 0..5 {
            let pt = PaintInformation {
                pos: [i as f32 * 10.0, 0.0],
                pressure: 0.5,
                ..Default::default()
            };
            stab.push(pt);
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
        let stab = registry.create_from_config(&config, 1.0);
        assert_eq!(stab.len(), 0);

        // "none" → pass-through.
        let config = StabilizerConfig {
            algorithm: "none".into(),
            params: vec![],
        };
        let stab = registry.create_from_config(&config, 1.0);
        assert_eq!(stab.len(), 0);

        // Known algorithm.
        let config = StabilizerConfig {
            algorithm: "laplacian".into(),
            params: vec![ParamValue::Float(0.5)],
        };
        let mut stab = registry.create_from_config(&config, 1.0);
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

    // ── Rendering against the stack ─────────────────────────────────────

    /// After every push, every rendered vertex is within the epsilon of its
    /// current position: the invariant the renderer relies on, checked over a
    /// fast wide curve at full strength where drift accumulates far behind
    /// the tip.
    #[test]
    fn rendered_positions_stay_within_epsilon() {
        let mut stab =
            stroke_stabilizer_stack(&StabilizerRegistry::new(), &laplacian_config(1.0), 0.0, 1.0);
        let mut diff = DivergenceDiff::new(DIVERGENCE_EPSILON);
        let mut rendered: Vec<[f32; 2]> = vec![];
        for i in 0..1200 {
            let th = i as f32 * 0.003;
            stab.push(mk(300.0 * th.cos(), 300.0 * th.sin(), i as f32 * 0.002));
            let cur = stab.stabilized();
            // Mirror the engine: re-render from the reported index, or
            // append the new vertices.
            let r = diff.update(cur, stab.max_divergence_window());
            let from = r.unwrap_or(rendered.len());
            rendered.truncate(from);
            rendered.extend(cur[from..].iter().map(|p| p.pos));
            for (j, (a, b)) in rendered.iter().zip(cur).enumerate() {
                let d = (a[0] - b.pos[0]).hypot(a[1] - b.pos[1]);
                assert!(
                    d < DIVERGENCE_EPSILON + 1e-3,
                    "push {i}: vertex {j} is rendered {d:.2} px from where it now lies"
                );
            }
        }
    }

    // ── Stroke stabilizer stack ─────────────────────────────────────────

    fn laplacian_config(strength: f32) -> StabilizerConfig {
        StabilizerConfig {
            algorithm: "laplacian".into(),
            params: vec![ParamValue::Float(strength)],
        }
    }

    /// Distance from the raw corner (40, 0) of an L path to the nearest
    /// stabilized vertex, with the path sampled every `step` px.
    fn l_corner_cut(step: f32) -> f32 {
        let mut stab =
            stroke_stabilizer_stack(&StabilizerRegistry::new(), &laplacian_config(0.8), 0.0, 1.0);
        let n = (40.0 / step).round() as usize;
        for i in 0..=n {
            stab.push(mk(i as f32 * step, 0.0, 0.0));
        }
        for i in 1..=n {
            stab.push(mk(40.0, i as f32 * step, 0.0));
        }
        stab.stabilized()
            .iter()
            .map(|p| dist(p.pos, [40.0, 0.0]))
            .fold(f32::INFINITY, f32::min)
    }

    /// Regression: the same path smooths the same whatever the pointer event
    /// rate. The Laplacian's reach is a count of vertices, so dense raw input
    /// (a high event rate, or a slow pen) used to shrink it to almost nothing.
    #[test]
    fn same_path_at_1x_and_8x_density_gives_the_same_geometry() {
        let sparse = l_corner_cut(10.0);
        let dense = l_corner_cut(1.25);
        assert!(
            sparse > 3.0 && dense > 3.0,
            "the corner must be smoothed at both densities: sparse {sparse}, dense {dense}"
        );
        assert!(
            (sparse - dense).abs() < 0.05,
            "corner cut must not depend on sample density: sparse {sparse}, dense {dense}"
        );
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
            .create("laplacian", &[ParamValue::Float(strength)], 1.0)
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
        let mut diff = DivergenceDiff::new(DIVERGENCE_EPSILON);
        for i in 0..8 {
            stab.push(mk(i as f32 * 10.0, 0.0, i as f32 * 0.01));
            diff.update(stab.stabilized(), stab.max_divergence_window());
        }
        // Straight-stroke prediction runs along +x (y≈0).
        assert!(stab.stabilized().last().unwrap().pos[1].abs() < 1e-2);
        let rendered_tip = stab.stabilized().len() - 1;

        // A turn downward.
        stab.push(mk(80.0, 30.0, 0.08));
        let real_len = 9; // 9 real points, N = 3 predicted
        let window = stab.max_divergence_window();

        let div = diff
            .update(stab.stabilized(), window)
            .expect("a turn must diverge");
        assert!(
            div <= real_len,
            "divergence {div} must cover the predicted tail (<= real_len {real_len})"
        );
        assert!(
            div >= rendered_tip.saturating_sub(window),
            "divergence {div} must stay within the widened window \
             (rendered tip {rendered_tip}, window {window})"
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
    /// window match a bare inner stabilizer, push for push.
    #[test]
    fn horizon_zero_is_transparent() {
        let mut pred = PredictingStabilizer::new(laplacian_inner(0.5), 0.0);
        let mut bare = laplacian_inner(0.5);
        for i in 0..8 {
            let p = mk(i as f32 * 10.0, (i as f32).sin() * 5.0, i as f32 * 0.01);
            pred.push(p);
            bare.push(p);
            assert_eq!(pred.stabilized(), bare.stabilized(), "step {i}");
        }
        assert_eq!(pred.max_divergence_window(), bare.max_divergence_window());
    }

    /// Regression: on fixed-spacing input the predicted tail spans the
    /// horizon at the current pen speed, with a constant point count. A
    /// cadence-derived count and a per-vertex step would freeze the tail at
    /// `count x spacing` whatever the speed.
    #[test]
    fn prediction_on_fixed_spacing_spans_horizon_at_current_speed() {
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 30.0);
        // 6 px vertices: 500 px/s for 12 vertices, then 2000 px/s.
        let (mut x, mut t) = (0.0f32, 0.0f32);
        let mut tail_at = |stab: &mut PredictingStabilizer, speed: f32, n: usize| {
            let mut tail = 0.0;
            for _ in 0..n {
                x += 6.0;
                t += 6.0 / speed;
                stab.push(mk(x, 0.0, t));
                let pts = stab.stabilized();
                let real = (x / 6.0).round() as usize;
                if real >= MIN_REAL_FOR_PREDICTION {
                    assert_eq!(
                        pts.len(),
                        real + PREDICTED_POINTS,
                        "constant predicted count"
                    );
                }
                tail = pts.last().unwrap().pos[0] - x;
            }
            tail
        };
        let slow = tail_at(&mut stab, 500.0, 12);
        let fast = tail_at(&mut stab, 2000.0, 12);
        assert!(
            (slow - 15.0).abs() < 3.0,
            "slow tail {slow}, expected ~15 px"
        );
        assert!(
            (fast - 60.0).abs() < 12.0,
            "fast tail {fast}, expected ~60 px"
        );
    }

    /// T-I: at stroke start the first < 3 real samples emit no prediction;
    /// once engaged, the polyline grows by exactly one per push and the
    /// divergence never reports outside the (ramping) window.
    #[test]
    fn stroke_start_ramps_without_breaking_growth() {
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 30.0);
        let mut diff = DivergenceDiff::new(DIVERGENCE_EPSILON);
        let mut prev_len = 0usize;
        for i in 0..12 {
            stab.push(mk(i as f32 * 10.0, 0.0, i as f32 * 0.01));
            let len = stab.stabilized().len();
            let rendered_tip = prev_len.saturating_sub(1);
            let window = stab.max_divergence_window();

            if let Some(div) = diff.update(stab.stabilized(), window) {
                assert!(
                    div >= rendered_tip.saturating_sub(window),
                    "step {i}: div {div} outside window (rendered tip {rendered_tip}, window {window})"
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

    // ── Batched diffs ───────────────────────────────────────────────────

    /// Regression: with several pushes between two diffs, every rendered
    /// vertex still ends within epsilon of where it now lies. Each frame here
    /// commits forty vertices at a strength whose window is three, so the
    /// vertices around the previous frame's tip, which the first push of the
    /// next frame reshapes, sit far behind the current tip.
    /// The whole stack (resampler, Laplacian and prediction) fed in batches
    /// leaves, after every batch, the polyline it leaves fed a sample at a
    /// time.
    #[test]
    fn batched_stack_matches_sequential() {
        let registry = StabilizerRegistry::new();
        let mut single = stroke_stabilizer_stack(&registry, &laplacian_config(0.6), 10.0, 1.0);
        let mut batched = stroke_stabilizer_stack(&registry, &laplacian_config(0.6), 10.0, 1.0);
        let raws: Vec<PaintInformation> = (0..300)
            .map(|i| {
                let th = i as f32 * 0.05;
                mk(
                    300.0 * th.cos() + i as f32,
                    200.0 * (2.0 * th).sin(),
                    i as f32 * 0.008,
                )
            })
            .collect();
        let mut start = 0;
        for size in (1..=12).cycle() {
            if start >= raws.len() {
                break;
            }
            let chunk = &raws[start..(start + size).min(raws.len())];
            for raw in chunk {
                single.push(*raw);
            }
            batched.push_all(chunk);
            start += chunk.len();
            assert_eq!(
                batched.stabilized(),
                single.stabilized(),
                "after the batch ending at sample {start}"
            );
        }
    }

    #[test]
    fn batched_pushes_keep_rendered_positions_within_epsilon() {
        let mut stab =
            stroke_stabilizer_stack(&StabilizerRegistry::new(), &laplacian_config(0.1), 0.0, 1.0);
        let mut diff = DivergenceDiff::new(DIVERGENCE_EPSILON);
        let mut rendered: Vec<[f32; 2]> = vec![];
        for i in 0..200 {
            // About 40 px of arc per push: several commits each.
            let th = i as f32 * 0.1;
            stab.push(mk(400.0 * th.cos(), 400.0 * th.sin(), i as f32 * 0.02));
            if i % 5 != 4 {
                continue;
            }
            let cur = stab.stabilized();
            let r = diff.update(cur, stab.max_divergence_window());
            let from = r.unwrap_or(rendered.len());
            rendered.truncate(from);
            rendered.extend(cur[from..].iter().map(|p| p.pos));
            for (j, (a, b)) in rendered.iter().zip(cur).enumerate() {
                let d = (a[0] - b.pos[0]).hypot(a[1] - b.pos[1]);
                assert!(
                    d < DIVERGENCE_EPSILON + 1e-3,
                    "push {i}: vertex {j} is rendered {d:.2} px from where it now lies"
                );
            }
        }
    }

    /// Regression: prediction copies only the part of the inner polyline a
    /// push can have changed, so its cost does not grow with the stroke. It
    /// used to copy the whole stroke on every push, with prediction on by
    /// default.
    #[test]
    fn prediction_copies_only_the_window_tail() {
        let mut stab = PredictingStabilizer::new(laplacian_inner(0.5), 30.0);
        let mut copies = Vec::new();
        for i in 0..400 {
            let before = stab.copied_vertices;
            stab.push(mk(
                i as f32 * 6.0,
                (i as f32 * 0.05).sin() * 40.0,
                i as f32 * 0.01,
            ));
            if i == 149 || i == 399 {
                copies.push(stab.copied_vertices - before);
            }
        }
        assert_eq!(
            copies[0], copies[1],
            "push 150 copied {} vertices, push 400 copied {}",
            copies[0], copies[1]
        );
        assert!(
            copies[1] as usize <= stab.inner.max_divergence_window() + 2,
            "a push copied {} vertices",
            copies[1]
        );
    }
}
