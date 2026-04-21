//! Interpolation of `PaintInformation` between pen samples.
//!
//! Provides both linear interpolation (lerp) and Catmull-Rom spline
//! interpolation for placing dabs at even spacing intervals between
//! raw input events.  Catmull-Rom produces smooth C1-continuous curves
//! through control points, eliminating the visible faceting that linear
//! interpolation causes on curved strokes.

use super::paint_info::PaintInformation;

/// Linearly interpolate all fields of two `PaintInformation` samples.
///
/// `t` is 0.0–1.0: 0 returns `a`, 1 returns `b`.
pub fn lerp_paint_info(a: &PaintInformation, b: &PaintInformation, t: f32) -> PaintInformation {
    PaintInformation {
        pos: lerp2(a.pos, b.pos, t),
        pressure: lerp(a.pressure, b.pressure, t),
        x_tilt: lerp(a.x_tilt, b.x_tilt, t),
        y_tilt: lerp(a.y_tilt, b.y_tilt, t),
        rotation: lerp(a.rotation, b.rotation, t),
        tangential_pressure: lerp(a.tangential_pressure, b.tangential_pressure, t),
        time: lerp(a.time, b.time, t),
        speed: lerp(a.speed, b.speed, t),
        distance: lerp(a.distance, b.distance, t),
        drawing_angle: lerp_angle(a.drawing_angle, b.drawing_angle, t),
        // Motion is a per-segment quantity — all dabs in a segment push in the
        // same direction, so we carry b's motion verbatim rather than blending
        // with the previous segment's.
        motion: b.motion,
        tilt_magnitude: lerp(a.tilt_magnitude, b.tilt_magnitude, t),
        tilt_direction: lerp_angle(a.tilt_direction, b.tilt_direction, t),
        // Index is not meaningful for interpolated points — use b's index.
        index: b.index,
        // Fade lerps with distance.
        fade: lerp(a.fade, b.fade, t),
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t)]
}

/// Lerp angles via shortest arc (handles wrapping around 2π).
#[inline]
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::TAU;
    let mut diff = (b - a) % TAU;
    if diff > std::f32::consts::PI {
        diff -= TAU;
    } else if diff < -std::f32::consts::PI {
        diff += TAU;
    }
    a + diff * t
}

// ── Catmull-Rom spline interpolation ──────────────────────────────────

/// Evaluate a Catmull-Rom spline at parameter `t` (0–1) for a scalar value.
///
/// Interpolates between `p1` and `p2` using `p0` and `p3` as outer
/// control points for tangent estimation.
#[inline]
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((-t3 + 2.0 * t2 - t) * p0
        + (3.0 * t3 - 5.0 * t2 + 2.0) * p1
        + (-3.0 * t3 + 4.0 * t2 + t) * p2
        + (t3 - t2) * p3)
}

/// Evaluate a Catmull-Rom spline at parameter `t` for a 2D position.
#[inline]
fn catmull_rom2(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    [
        catmull_rom(p0[0], p1[0], p2[0], p3[0], t),
        catmull_rom(p0[1], p1[1], p2[1], p3[1], t),
    ]
}

/// Evaluate a Catmull-Rom spline for an angle value, handling wrapping.
///
/// Unwraps angles relative to `p1` before interpolation to avoid
/// discontinuities at the ±π boundary.
#[inline]
fn catmull_rom_angle(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    // Unwrap all angles relative to p1.
    let unwrap = |a: f32, ref_: f32| -> f32 {
        let mut d = (a - ref_) % TAU;
        if d > PI { d -= TAU; }
        else if d < -PI { d += TAU; }
        ref_ + d
    };
    let u0 = unwrap(p0, p1);
    let u2 = unwrap(p2, p1);
    let u3 = unwrap(p3, p1);
    catmull_rom(u0, p1, u2, u3, t)
}

/// Interpolate all fields of `PaintInformation` along a Catmull-Rom spline.
///
/// Interpolates between `p1` and `p2` at parameter `t` (0–1), using
/// `p0` and `p3` as outer control points.  Clamps pressure and
/// tilt_magnitude to [0, 1] to prevent overshoot.
pub fn catmull_rom_paint_info(
    p0: &PaintInformation,
    p1: &PaintInformation,
    p2: &PaintInformation,
    p3: &PaintInformation,
    t: f32,
) -> PaintInformation {
    PaintInformation {
        pos: catmull_rom2(p0.pos, p1.pos, p2.pos, p3.pos, t),
        pressure: catmull_rom(p0.pressure, p1.pressure, p2.pressure, p3.pressure, t).clamp(0.0, 1.0),
        x_tilt: catmull_rom(p0.x_tilt, p1.x_tilt, p2.x_tilt, p3.x_tilt, t),
        y_tilt: catmull_rom(p0.y_tilt, p1.y_tilt, p2.y_tilt, p3.y_tilt, t),
        rotation: catmull_rom_angle(p0.rotation, p1.rotation, p2.rotation, p3.rotation, t),
        tangential_pressure: catmull_rom(
            p0.tangential_pressure, p1.tangential_pressure,
            p2.tangential_pressure, p3.tangential_pressure, t,
        ).clamp(0.0, 1.0),
        time: catmull_rom(p0.time, p1.time, p2.time, p3.time, t),
        speed: catmull_rom(p0.speed, p1.speed, p2.speed, p3.speed, t).clamp(0.0, 1.0),
        distance: catmull_rom(p0.distance, p1.distance, p2.distance, p3.distance, t),
        drawing_angle: catmull_rom_angle(
            p0.drawing_angle, p1.drawing_angle, p2.drawing_angle, p3.drawing_angle, t,
        ),
        // Same rationale as the lerp path: motion is per-segment, not per-dab.
        motion: p2.motion,
        tilt_magnitude: catmull_rom(
            p0.tilt_magnitude, p1.tilt_magnitude, p2.tilt_magnitude, p3.tilt_magnitude, t,
        ).clamp(0.0, 1.0),
        tilt_direction: catmull_rom_angle(
            p0.tilt_direction, p1.tilt_direction, p2.tilt_direction, p3.tilt_direction, t,
        ),
        index: p2.index,
        fade: catmull_rom(p0.fade, p1.fade, p2.fade, p3.fade, t),
    }
}

// ── Arc-length parameterized Catmull-Rom segment ─────────────────────

/// Number of sub-chords used to approximate arc length.
const ARC_LEN_SUBDIVISIONS: usize = 8;

/// A Catmull-Rom spline segment with a precomputed arc-length lookup table.
///
/// Stores four control points and a cumulative chord-length table so that
/// dabs can be placed at uniform *distance* intervals along the curve
/// rather than at uniform parametric `t` intervals.
pub struct CatmullRomSegment<'a> {
    p0: &'a PaintInformation,
    p1: &'a PaintInformation,
    p2: &'a PaintInformation,
    p3: &'a PaintInformation,
    /// Cumulative chord lengths at t = 0, 1/N, 2/N, …, 1.
    /// Length = ARC_LEN_SUBDIVISIONS + 1.
    cumulative: [f32; ARC_LEN_SUBDIVISIONS + 1],
}

impl<'a> CatmullRomSegment<'a> {
    /// Build a segment from four control points, precomputing the arc-length LUT.
    pub fn new(
        p0: &'a PaintInformation,
        p1: &'a PaintInformation,
        p2: &'a PaintInformation,
        p3: &'a PaintInformation,
    ) -> Self {
        let mut cumulative = [0.0f32; ARC_LEN_SUBDIVISIONS + 1];
        let mut prev_pos = p1.pos;
        for i in 1..=ARC_LEN_SUBDIVISIONS {
            let t = i as f32 / ARC_LEN_SUBDIVISIONS as f32;
            let pos = catmull_rom2(p0.pos, p1.pos, p2.pos, p3.pos, t);
            let dx = pos[0] - prev_pos[0];
            let dy = pos[1] - prev_pos[1];
            cumulative[i] = cumulative[i - 1] + (dx * dx + dy * dy).sqrt();
            prev_pos = pos;
        }
        Self { p0, p1, p2, p3, cumulative }
    }

    /// Total arc length of this segment.
    pub fn arc_length(&self) -> f32 {
        self.cumulative[ARC_LEN_SUBDIVISIONS]
    }

    /// Evaluate the spline at a given arc-length distance from the start.
    ///
    /// Maps `distance` to a parametric `t` via the LUT, then evaluates
    /// the Catmull-Rom spline for all `PaintInformation` fields.
    pub fn eval_at_distance(&self, distance: f32) -> PaintInformation {
        let t = self.distance_to_t(distance);
        catmull_rom_paint_info(self.p0, self.p1, self.p2, self.p3, t)
    }

    /// Map an arc-length distance to a parametric `t` using the LUT.
    fn distance_to_t(&self, distance: f32) -> f32 {
        let total = self.arc_length();
        if total < 1e-6 {
            return 0.0;
        }
        let d = distance.clamp(0.0, total);

        // Linear search (N=8, so this is fast).
        for i in 1..=ARC_LEN_SUBDIVISIONS {
            if self.cumulative[i] >= d {
                let seg_start = self.cumulative[i - 1];
                let seg_end = self.cumulative[i];
                let seg_len = seg_end - seg_start;
                let frac = if seg_len > 1e-6 { (d - seg_start) / seg_len } else { 0.0 };
                let t_start = (i - 1) as f32 / ARC_LEN_SUBDIVISIONS as f32;
                let t_step = 1.0 / ARC_LEN_SUBDIVISIONS as f32;
                return t_start + frac * t_step;
            }
        }
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_midpoint() {
        let a = PaintInformation {
            pos: [0.0, 0.0],
            pressure: 0.2,
            ..Default::default()
        };
        let b = PaintInformation {
            pos: [100.0, 200.0],
            pressure: 0.8,
            ..Default::default()
        };
        let mid = lerp_paint_info(&a, &b, 0.5);
        assert!((mid.pos[0] - 50.0).abs() < 1e-6);
        assert!((mid.pos[1] - 100.0).abs() < 1e-6);
        assert!((mid.pressure - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lerp_endpoints() {
        let a = PaintInformation { pressure: 0.3, ..Default::default() };
        let b = PaintInformation { pressure: 0.9, ..Default::default() };
        let at_a = lerp_paint_info(&a, &b, 0.0);
        let at_b = lerp_paint_info(&a, &b, 1.0);
        assert!((at_a.pressure - 0.3).abs() < 1e-6);
        assert!((at_b.pressure - 0.9).abs() < 1e-6);
    }

    #[test]
    fn angle_wrapping() {
        use std::f32::consts::PI;
        // From near 2π to near 0 — should go the short way.
        let result = lerp_angle(PI * 1.9, PI * 0.1, 0.5);
        // Midpoint should be near 0/2π, not near π.
        assert!(result.abs() < 0.5 || (result - std::f32::consts::TAU).abs() < 0.5);
    }

    // ── Catmull-Rom tests ────────────────────────────────────────────

    fn pt(x: f32, y: f32) -> PaintInformation {
        PaintInformation { pos: [x, y], ..Default::default() }
    }

    fn pt_full(x: f32, y: f32, pressure: f32) -> PaintInformation {
        PaintInformation { pos: [x, y], pressure, ..Default::default() }
    }

    #[test]
    fn catmull_rom_collinear_matches_lerp() {
        // Four collinear points — CR should produce the same result as lerp
        // between p1 and p2 (within floating-point tolerance).
        let p0 = pt(0.0, 0.0);
        let p1 = pt(10.0, 10.0);
        let p2 = pt(20.0, 20.0);
        let p3 = pt(30.0, 30.0);

        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let cr = catmull_rom_paint_info(&p0, &p1, &p2, &p3, t);
            let lr = lerp_paint_info(&p1, &p2, t);
            assert!((cr.pos[0] - lr.pos[0]).abs() < 1e-4, "x mismatch at t={t}");
            assert!((cr.pos[1] - lr.pos[1]).abs() < 1e-4, "y mismatch at t={t}");
        }
    }

    #[test]
    fn catmull_rom_right_angle_produces_curve() {
        // A right-angle turn: (0,0) → (10,0) → (10,10).
        // With CR, the midpoint (t=0.5) should NOT be on the straight line
        // from (10,0) to (10,10) — it should curve inward.
        let p0 = pt(0.0, 0.0);
        let p1 = pt(10.0, 0.0);
        let p2 = pt(10.0, 10.0);
        let p3 = pt(0.0, 10.0);

        let mid = catmull_rom_paint_info(&p0, &p1, &p2, &p3, 0.5);
        // On a straight line from p1 to p2, x would be exactly 10.0.
        // The CR curve should pull it away from 10.0.
        assert!(
            (mid.pos[0] - 10.0).abs() > 0.1,
            "midpoint should curve away from straight line, got x={}",
            mid.pos[0]
        );
    }

    #[test]
    fn catmull_rom_endpoints_match() {
        // At t=0 the result should equal p1, at t=1 it should equal p2.
        let p0 = pt(0.0, 5.0);
        let p1 = pt(10.0, 20.0);
        let p2 = pt(30.0, 40.0);
        let p3 = pt(50.0, 35.0);

        let at_0 = catmull_rom_paint_info(&p0, &p1, &p2, &p3, 0.0);
        let at_1 = catmull_rom_paint_info(&p0, &p1, &p2, &p3, 1.0);
        assert!((at_0.pos[0] - 10.0).abs() < 1e-5);
        assert!((at_0.pos[1] - 20.0).abs() < 1e-5);
        assert!((at_1.pos[0] - 30.0).abs() < 1e-5);
        assert!((at_1.pos[1] - 40.0).abs() < 1e-5);
    }

    #[test]
    fn catmull_rom_pressure_clamped() {
        // Arrange points so CR overshoots pressure beyond [0,1].
        let p0 = pt_full(0.0, 0.0, 0.0);
        let p1 = pt_full(10.0, 0.0, 0.0);
        let p2 = pt_full(20.0, 0.0, 1.0);
        let p3 = pt_full(30.0, 0.0, 1.0);

        // Sample at many t values — pressure should always be in [0, 1].
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let result = catmull_rom_paint_info(&p0, &p1, &p2, &p3, t);
            assert!(
                result.pressure >= 0.0 && result.pressure <= 1.0,
                "pressure {} out of [0,1] at t={t}",
                result.pressure,
            );
        }
    }

    #[test]
    fn catmull_rom_duplicate_endpoint_no_nan() {
        // Duplicate endpoints (p0=p1, p3=p2) — should not produce NaN.
        let p1 = pt(10.0, 20.0);
        let p2 = pt(30.0, 40.0);

        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let result = catmull_rom_paint_info(&p1, &p1, &p2, &p2, t);
            assert!(!result.pos[0].is_nan(), "NaN at t={t}");
            assert!(!result.pos[1].is_nan(), "NaN at t={t}");
        }
    }

    #[test]
    fn arc_length_straight_line() {
        // For a straight line, arc length should equal Euclidean distance.
        let p0 = pt(0.0, 0.0);
        let p1 = pt(10.0, 0.0);
        let p2 = pt(20.0, 0.0);
        let p3 = pt(30.0, 0.0);

        let seg = CatmullRomSegment::new(&p0, &p1, &p2, &p3);
        let expected = 10.0; // distance from p1 to p2
        assert!(
            (seg.arc_length() - expected).abs() < 0.01,
            "arc_length {} should be ~{expected}",
            seg.arc_length()
        );
    }

    #[test]
    fn arc_length_eval_at_distance_endpoints() {
        let p0 = pt(0.0, 0.0);
        let p1 = pt(10.0, 0.0);
        let p2 = pt(10.0, 10.0);
        let p3 = pt(0.0, 10.0);

        let seg = CatmullRomSegment::new(&p0, &p1, &p2, &p3);

        // At distance 0, should be at p1.
        let start = seg.eval_at_distance(0.0);
        assert!((start.pos[0] - 10.0).abs() < 0.1);
        assert!((start.pos[1] - 0.0).abs() < 0.1);

        // At full arc length, should be at p2.
        let end = seg.eval_at_distance(seg.arc_length());
        assert!((end.pos[0] - 10.0).abs() < 0.1);
        assert!((end.pos[1] - 10.0).abs() < 0.1);
    }
}
