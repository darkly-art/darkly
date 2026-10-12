//! Arc-length resampling ahead of a stabilizer algorithm.
//!
//! Stabilizer algorithms work on vertex indices: the Laplacian's reach is a
//! number of sweeps, one vertex each. Raw pointer samples arrive at whatever
//! rate the platform delivers, so their spacing depends on the event rate and
//! the pen speed, and so would the reach in pixels. [`ResamplingStabilizer`]
//! feeds the inner algorithm vertices at a fixed arc-length spacing along the
//! raw path instead, which makes an index mean the same distance on every
//! platform, at every pen speed and zoom.
//!
//! The inner algorithm's polyline is the committed vertices followed by the
//! latest raw sample as a provisional tip, so the algorithm pins the pen
//! position itself and smooths right up to it. Until the pen has travelled
//! one spacing past the last committed vertex, each batch retracts that
//! provisional tip from the inner and pushes the new one in its place.
//!
//! A batch of raw samples reaches the inner as at most one retract and one
//! `push_all` of every vertex it commits plus the new provisional tip; the
//! provisional tips between its samples never reach the inner. The inner's
//! raw sequence is the one pushing the samples one at a time leaves.
//!
//! Guarantees of the output polyline:
//! - Committed vertices lie on the raw polyline, `spacing` apart in raw arc
//!   length, every [`PaintInformation`] field interpolated between the two
//!   raw samples a vertex falls between.
//! - The last vertex is the latest raw sample: the tip is pinned at the pen
//!   with zero lag.
//! - The polyline never shrinks and never ends in a zero-length segment.
//! - At most [`MAX_COMMITS_PER_SAMPLE`] vertices commit per raw sample. A raw segment
//!   longer than that many spacings gets exactly that many vertices spread
//!   evenly along it, the last one on the raw sample, so on such a segment
//!   the spacing stretches. This is the one place the output still depends
//!   on input density, and it only arises for jumps of more than
//!   `MAX_COMMITS_PER_SAMPLE x spacing` in a single event.
//!
//! Divergence window: the inner algorithm's window `W` bounds each of its
//! batches relative to its own tip before that batch. After the provisional
//! tip is retracted, the batch's first vertex lands where that tip was, so the
//! earliest vertex it can move is `W` behind the resampler's tip before the
//! raw batch. The resampler's window is therefore the inner's, measured, like
//! every stabilizer window, from the tip before the batch, which is how the
//! stroke engine diffs: from the tip as last rendered, however many vertices
//! committed since.

use super::interpolation::lerp_paint_info;
use super::paint_info::PaintInformation;
use super::stabilizer::StabilizerAlgorithm;

/// Arc length between committed vertices, in CSS pixels of pen travel.
pub const RESAMPLE_SPACING_CSS_PX: f32 = 6.0;

/// Most vertices one raw sample may commit, so a single jump cannot flood the
/// polyline with vertices.
pub const MAX_COMMITS_PER_SAMPLE: usize = 8;

/// Raw segments shorter than this carry no direction and commit nothing.
const MIN_SEGMENT_PX: f32 = 1.0e-4;

/// Resamples raw pointer samples to a fixed arc-length spacing and feeds the
/// result to an inner stabilizer algorithm. See the module docs.
pub struct ResamplingStabilizer {
    inner: Box<dyn StabilizerAlgorithm>,
    /// Arc length between committed vertices, canvas px.
    spacing: f32,
    /// The previous raw sample: the start of the raw segment being walked.
    last_raw: Option<PaintInformation>,
    /// Raw arc length from the last committed vertex to `last_raw`.
    residual: f32,
    /// The last committed vertex is the raw tip itself, so no provisional tip
    /// follows it in the inner polyline.
    tip_is_committed: bool,
    /// The vertices a batch hands the inner, reused across batches.
    batch: Vec<PaintInformation>,
}

impl ResamplingStabilizer {
    /// Wrap `inner`, committing a vertex every `spacing` canvas px of raw arc
    /// length.
    pub fn new(inner: Box<dyn StabilizerAlgorithm>, spacing: f32) -> Self {
        debug_assert!(
            spacing > 0.0,
            "resample spacing must be positive: {spacing}"
        );
        Self {
            inner,
            spacing,
            last_raw: None,
            residual: 0.0,
            tip_is_committed: false,
            batch: Vec::new(),
        }
    }

    /// Commit the vertices the raw segment `from -> to` crosses, and advance
    /// the arc length carried to the next segment.
    fn commit_segment(&mut self, from: PaintInformation, to: PaintInformation) {
        let seg = (to.pos[0] - from.pos[0]).hypot(to.pos[1] - from.pos[1]);
        if seg < MIN_SEGMENT_PX {
            return;
        }
        // Arc length along this segment of each vertex it crosses.
        let first = self.spacing - self.residual;
        let crossed = if first > seg {
            0
        } else {
            ((seg - first) / self.spacing).floor() as usize + 1
        };
        if crossed > MAX_COMMITS_PER_SAMPLE {
            // Too long to commit at the nominal spacing: spread the cap
            // evenly, ending on the raw sample.
            for j in 1..=MAX_COMMITS_PER_SAMPLE {
                let t = j as f32 / MAX_COMMITS_PER_SAMPLE as f32;
                self.batch.push(lerp_paint_info(&from, &to, t));
            }
            self.residual = 0.0;
            self.tip_is_committed = true;
            return;
        }
        for j in 0..crossed {
            let d = first + j as f32 * self.spacing;
            self.batch.push(lerp_paint_info(&from, &to, d / seg));
        }
        self.residual = seg - (first + crossed as f32 * self.spacing - self.spacing);
        self.tip_is_committed = self.residual < MIN_SEGMENT_PX;
    }
}

impl StabilizerAlgorithm for ResamplingStabilizer {
    fn push_all(&mut self, raws: &[PaintInformation]) {
        let Some(&tip) = raws.last() else {
            return;
        };
        if self.last_raw.is_some() && !self.tip_is_committed {
            self.inner.retract_tip();
        }
        self.batch.clear();
        for &raw in raws {
            match self.last_raw {
                // The stroke origin is the first committed vertex.
                None => {
                    self.batch.push(raw);
                    self.residual = 0.0;
                    self.tip_is_committed = true;
                }
                Some(from) => self.commit_segment(from, raw),
            }
            self.last_raw = Some(raw);
        }
        if !self.tip_is_committed {
            self.batch.push(tip);
        }
        // Empty only when the tip was already committed and every sample
        // stood still on it: nothing was retracted and nothing changed.
        if !self.batch.is_empty() {
            self.inner.push_all(&self.batch);
        }
    }

    fn retract_tip(&mut self) {
        unreachable!("the resampler is the outermost stage that replaces a tip");
    }

    fn stabilized(&self) -> &[PaintInformation] {
        self.inner.stabilized()
    }

    fn max_divergence_window(&self) -> usize {
        self.inner.max_divergence_window()
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.last_raw = None;
        self.residual = 0.0;
        self.tip_is_committed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::stabilizer::StabilizerRegistry;
    use crate::brush::stroke_engine::{DivergenceDiff, DIVERGENCE_EPSILON};
    use crate::gpu::params::ParamValue;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const SPACING: f32 = 6.0;

    /// Inner algorithm that keeps committed vertices verbatim and counts its
    /// pushes, so a test can see exactly what the resampler fed it.
    struct CountingInner {
        points: Vec<PaintInformation>,
        pushes: Arc<AtomicUsize>,
    }

    impl StabilizerAlgorithm for CountingInner {
        fn push_all(&mut self, points: &[PaintInformation]) {
            self.pushes.fetch_add(1, Ordering::Relaxed);
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

    fn counting() -> (ResamplingStabilizer, Arc<AtomicUsize>) {
        let pushes = Arc::new(AtomicUsize::new(0));
        let inner = CountingInner {
            points: Vec::new(),
            pushes: pushes.clone(),
        };
        (ResamplingStabilizer::new(Box::new(inner), SPACING), pushes)
    }

    fn laplacian(strength: f32) -> ResamplingStabilizer {
        let inner = StabilizerRegistry::new()
            .create("laplacian", &[ParamValue::Float(strength)], 1.0)
            .expect("laplacian registered");
        ResamplingStabilizer::new(inner, SPACING)
    }

    fn mk(x: f32, y: f32, pressure: f32, time: f32) -> PaintInformation {
        PaintInformation {
            pos: [x, y],
            pressure,
            time,
            ..Default::default()
        }
    }

    fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }

    /// A batch of raw samples reaches the inner as one push of every vertex
    /// it commits plus the new provisional tip, and leaves the inner polyline
    /// that pushing the samples one at a time leaves. A batch that stands
    /// still on a committed tip reaches the inner not at all.
    #[test]
    fn a_batch_reaches_the_inner_as_one_push() {
        // Steps from under a spacing up past the commit cap.
        let raws: Vec<PaintInformation> = (0..50)
            .map(|i| {
                let s = i as f32;
                mk(s * s * 0.8, (s * 0.3).sin() * 20.0, 0.5, s * 0.01)
            })
            .collect();
        let (mut single, _) = counting();
        let (mut batched, pushes) = counting();
        for chunk in raws.chunks(7) {
            for raw in chunk {
                single.push(*raw);
            }
            let before = pushes.load(Ordering::Relaxed);
            batched.push_all(chunk);
            assert_eq!(pushes.load(Ordering::Relaxed) - before, 1);
            assert_eq!(batched.stabilized(), single.stabilized());
        }

        let (mut still, pushes) = counting();
        let origin = mk(10.0, 10.0, 0.5, 0.0);
        still.push_all(&[origin]);
        let before = (pushes.load(Ordering::Relaxed), still.stabilized().to_vec());
        still.push_all(&[origin, origin]);
        assert_eq!(
            (pushes.load(Ordering::Relaxed), still.stabilized().to_vec()),
            before,
            "standing still on a committed tip changed the inner"
        );
    }

    /// The tip is always the raw sample itself, the inner polyline holds the
    /// committed vertices plus that one provisional tip, and nothing shrinks.
    #[test]
    fn tip_is_pinned_and_inner_holds_committed_plus_tip() {
        let (mut stab, _pushes) = counting();
        let steps = [0.3, 25.0, 1.7, 4.4, 0.9, 13.1, 2.2, 7.7, 0.5, 19.3, 3.3];
        let (mut x, mut y, mut arc) = (0.0f32, 0.0f32, 0.0f32);
        let mut prev_len = 0;
        for (i, &step) in steps.iter().cycle().take(60).enumerate() {
            let angle = i as f32 * 0.4;
            x += step * angle.cos();
            y += step * angle.sin();
            if i > 0 {
                arc += step;
            }
            let raw = mk(x, y, 0.1 + (i % 7) as f32 * 0.1, i as f32 * 0.004);
            stab.push(raw);

            let out = stab.stabilized();
            let tip = out.last().unwrap();
            assert!(
                dist(tip.pos, raw.pos) < 1e-3,
                "push {i}: tip must sit on the pen"
            );
            assert!(
                (tip.pressure - raw.pressure).abs() < 1e-4,
                "push {i}: tip pressure"
            );
            assert!((tip.time - raw.time).abs() < 1e-5, "push {i}: tip time");

            // One committed vertex at the origin, then one per spacing of
            // raw arc length (no step here reaches the per-push cap), plus
            // the provisional tip unless the pen sits exactly on a commit.
            let committed = 1 + (arc / SPACING).floor() as usize;
            assert!(
                out.len() == committed + 1 || out.len() == committed,
                "push {i}: {} vertices for {committed} committed",
                out.len()
            );
            assert!(out.len() >= prev_len, "push {i}: output must never shrink");
            prev_len = out.len();
        }
    }

    /// Committed vertices are one spacing apart along the raw path, with the
    /// sensors interpolated at the vertex's own position.
    #[test]
    fn committed_vertices_sit_at_fixed_spacing_with_interpolated_sensors() {
        let mut stab = laplacian(0.0);
        let total = 2.7 * 100.0;
        for i in 0..=100 {
            let x = i as f32 * 2.7;
            stab.push(mk(x, 0.0, x / total, x / total));
        }
        let out = stab.stabilized();
        let committed = &out[..out.len() - 1];
        for (j, v) in committed.iter().enumerate() {
            let expected = j as f32 * SPACING;
            assert!(
                (v.pos[0] - expected).abs() < 1e-3,
                "vertex {j} at {}",
                v.pos[0]
            );
            assert!(
                (v.pressure - expected / total).abs() < 1e-4,
                "vertex {j} pressure"
            );
            assert!((v.time - expected / total).abs() < 1e-4, "vertex {j} time");
        }
    }

    /// No push moves a vertex further behind the tip as it stood before the
    /// push than the window, which is the inner algorithm's: checked per push
    /// and across batches of pushes, as the stroke engine diffs once per
    /// frame, on a path whose every fifth sample jumps several spacings.
    #[test]
    fn pushes_move_nothing_further_than_the_window_behind_the_earlier_tip() {
        let mut stab = laplacian(1.0);
        assert_eq!(stab.max_divergence_window(), 161);
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut per_push: Vec<PaintInformation> = vec![];
        let mut per_batch: Vec<PaintInformation> = vec![];
        for i in 0..600 {
            let step = if i % 5 == 0 { 40.0 } else { 1.0 };
            let angle = i as f32 * 0.07;
            x += step * angle.cos();
            y += step * angle.sin();
            stab.push(mk(x, y, 0.5, i as f32 * 0.004));
            let cur = stab.stabilized();
            for (earlier, what) in [(&per_push, "push"), (&per_batch, "batch")] {
                let settled = earlier
                    .len()
                    .saturating_sub(stab.max_divergence_window() + 1);
                assert_eq!(
                    &cur[..settled],
                    &earlier[..settled],
                    "{what} ending at {i} moved a vertex behind the window"
                );
            }
            per_push = cur.to_vec();
            if i % 7 == 6 {
                per_batch = cur.to_vec();
            }
        }
    }

    /// The stroke origin alone is a one-vertex polyline, and a second sample
    /// inside the first spacing appends the tip without disturbing it.
    #[test]
    fn stroke_start_appends_without_divergence() {
        let mut stab = laplacian(0.5);
        let mut diff = DivergenceDiff::new(DIVERGENCE_EPSILON);
        assert_eq!(stab.max_divergence_window(), 41);
        stab.push(mk(10.0, 10.0, 0.5, 0.0));
        assert_eq!(stab.stabilized().len(), 1);
        assert_eq!(
            diff.update(stab.stabilized(), 41),
            None,
            "nothing was rendered before the origin"
        );

        stab.push(mk(12.0, 10.0, 0.5, 0.004));
        assert_eq!(stab.stabilized().len(), 2);
        assert_eq!(
            diff.update(stab.stabilized(), 41),
            None,
            "the origin did not move"
        );

        // Replacing the tip in place is a divergence at the tip's own index.
        stab.push(mk(14.0, 11.0, 0.5, 0.008));
        assert_eq!(stab.stabilized().len(), 2);
        assert_eq!(diff.update(stab.stabilized(), 41), Some(1));
    }

    /// Regression: committing a vertex must not jolt the stroke. On a tight
    /// curve at full strength, the vertex behind the tip moves about as much
    /// on a push that commits as on one that only replaces the tip. With
    /// the tip outside the smoothed polyline, or with a smoother that slides
    /// points along the stroke, commits moved it several times further.
    #[test]
    fn commits_reshape_no_more_than_tip_moves() {
        let mut stab = laplacian(1.0);
        let mut prev: Vec<[f32; 2]> = vec![];
        let (mut on_commit, mut on_replace) = (0.0f32, 0.0f32);
        for i in 0..600 {
            let th = i as f32 * 0.012;
            stab.push(mk(30.0 * th.cos(), 30.0 * th.sin(), 0.5, i as f32 * 0.004));
            let cur: Vec<[f32; 2]> = stab.stabilized().iter().map(|p| p.pos).collect();
            if i > 100 {
                let j = prev.len() - 2;
                let moved = dist(prev[j], cur[j]);
                if cur.len() > prev.len() {
                    on_commit = on_commit.max(moved);
                } else {
                    on_replace = on_replace.max(moved);
                }
            }
            prev = cur;
        }
        assert!(
            on_commit < on_replace * 1.5,
            "a commit moved the vertex behind the tip {on_commit:.2} px, a replace at most {on_replace:.2} px"
        );
    }

    /// A jump longer than the cap allows commits exactly the cap, spread
    /// evenly and ending on the raw sample, with no duplicate tip; the next
    /// commit is a nominal spacing past that last vertex.
    #[test]
    fn long_jump_commits_the_cap_evenly_and_ends_on_the_sample() {
        let (mut stab, _pushes) = counting();
        stab.push(mk(0.0, 0.0, 0.5, 0.0));
        stab.push(mk(200.0, 0.0, 0.5, 0.01));
        let out = stab.stabilized();
        assert_eq!(out.len(), 1 + MAX_COMMITS_PER_SAMPLE, "no extra tip vertex");
        for (j, v) in out.iter().enumerate() {
            let expected = j as f32 * 200.0 / MAX_COMMITS_PER_SAMPLE as f32;
            assert!(
                (v.pos[0] - expected).abs() < 1e-3,
                "vertex {j} at {}",
                v.pos[0]
            );
        }

        stab.push(mk(203.0, 0.0, 0.5, 0.02));
        let out = stab.stabilized();
        assert_eq!(
            out.len(),
            2 + MAX_COMMITS_PER_SAMPLE,
            "provisional tip appended"
        );
        assert!((out.last().unwrap().pos[0] - 203.0).abs() < 1e-3);
        stab.push(mk(207.0, 0.0, 0.5, 0.03));
        let out = stab.stabilized();
        assert_eq!(
            out.len(),
            3 + MAX_COMMITS_PER_SAMPLE,
            "one commit, tip replaced"
        );
        let next = out[out.len() - 2].pos[0];
        assert!(
            (next - 206.0).abs() < 1e-3,
            "next commit at {next}, expected 206"
        );
        assert!((out.last().unwrap().pos[0] - 207.0).abs() < 1e-3);
    }
}
