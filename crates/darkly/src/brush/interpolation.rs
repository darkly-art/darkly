//! Linear interpolation of `PaintInformation` between stabilized vertices.
//!
//! Dabs are placed on the straight segment between consecutive vertices,
//! with every field interpolated along it.

use super::paint_info::PaintInformation;

/// Linearly interpolate all fields of two `PaintInformation` samples.
///
/// `t` is 0.0-1.0: 0 returns `a`, 1 returns `b`.
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
        // Motion is filled by `StrokeEngine::place_dab` from the previous-dab
        // delta; interpolators have no view of dab order, so they leave it zero.
        motion: [0.0, 0.0],
        tilt_magnitude: lerp(a.tilt_magnitude, b.tilt_magnitude, t),
        tilt_direction: lerp_angle(a.tilt_direction, b.tilt_direction, t),
        // Index is not meaningful for interpolated points, so use b's index.
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

/// Shortest signed difference `b - a`, wrapped to (−π, π].
///
/// The one wrap implementation: angle lerping and the stroke engine's
/// stamp-orientation tracker both route through it.
#[inline]
pub fn shortest_angle_diff(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut diff = (b - a) % TAU;
    if diff > PI {
        diff -= TAU;
    } else if diff < -PI {
        diff += TAU;
    }
    diff
}

/// Lerp angles via shortest arc (handles wrapping around 2π).
#[inline]
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    a + shortest_angle_diff(a, b) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: interpolators must NOT carry `motion` from either
    /// endpoint. `PaintInformation.motion` is per-dab and owned by
    /// `StrokeEngine::place_dab`; interpolation runs before dab order is
    /// known, so propagating it would re-introduce the segment-level
    /// motion bug that broke smudge sampling.
    #[test]
    fn lerp_leaves_motion_zero() {
        let a = PaintInformation {
            motion: [99.0, 99.0],
            ..Default::default()
        };
        let b = PaintInformation {
            motion: [42.0, -7.0],
            ..Default::default()
        };
        let mid = lerp_paint_info(&a, &b, 0.5);
        assert_eq!(mid.motion, [0.0, 0.0]);
    }

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
        let a = PaintInformation {
            pressure: 0.3,
            ..Default::default()
        };
        let b = PaintInformation {
            pressure: 0.9,
            ..Default::default()
        };
        let at_a = lerp_paint_info(&a, &b, 0.0);
        let at_b = lerp_paint_info(&a, &b, 1.0);
        assert!((at_a.pressure - 0.3).abs() < 1e-6);
        assert!((at_b.pressure - 0.9).abs() < 1e-6);
    }

    #[test]
    fn angle_wrapping() {
        use std::f32::consts::PI;
        // From near 2π to near 0, it should go the short way.
        let result = lerp_angle(PI * 1.9, PI * 0.1, 0.5);
        // Midpoint should be near 0/2π, not near π.
        assert!(result.abs() < 0.5 || (result - std::f32::consts::TAU).abs() < 0.5);
    }

    /// The one wrap implementation behind `lerp_angle` and the stroke
    /// engine's orientation tracker: always the
    /// shortest signed arc, always within (−π, π].
    #[test]
    fn shortest_angle_diff_wraps_at_pi() {
        use std::f32::consts::{PI, TAU};

        assert!((shortest_angle_diff(0.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((shortest_angle_diff(0.5, 0.0) + 0.5).abs() < 1e-6);

        // Across the wrap: 0.1 rad short of a full turn is −0.1, not +6.18.
        assert!((shortest_angle_diff(0.0, TAU - 0.1) + 0.1).abs() < 1e-5);
        assert!((shortest_angle_diff(TAU - 0.1, 0.0) - 0.1).abs() < 1e-5);

        // Multiple turns of winding collapse to the same short arc.
        assert!((shortest_angle_diff(0.0, TAU * 3.0 + 0.25) - 0.25).abs() < 1e-4);

        // Never leaves (−π, π], including at the antipode.
        for i in 0..64 {
            let b = -TAU * 2.0 + i as f32 * (TAU * 4.0 / 64.0);
            let d = shortest_angle_diff(0.7, b);
            assert!(d > -PI - 1e-5 && d <= PI + 1e-5, "diff {d} out of range");
        }
    }
}
