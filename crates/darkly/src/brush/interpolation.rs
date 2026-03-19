//! Linear interpolation of `PaintInformation` between two pen samples.
//!
//! Used by the stroke engine to place dabs at even spacing intervals
//! between raw input events.  All fields are lerped — positions, pressure,
//! tilt, time, and derived values.

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
        tilt_magnitude: lerp(a.tilt_magnitude, b.tilt_magnitude, t),
        tilt_direction: lerp_angle(a.tilt_direction, b.tilt_direction, t),
        // Index is not meaningful for interpolated points — use b's index.
        index: b.index,
        // Fuzzy values are set per-dab by the stroke engine, not interpolated.
        fuzzy_dab: 0.0,
        fuzzy_stroke: a.fuzzy_stroke,
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
}
