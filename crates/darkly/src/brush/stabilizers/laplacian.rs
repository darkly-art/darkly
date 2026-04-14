//! Laplacian relaxation stabilizer — iterative smoothing with zero lag.
//!
//! Maintains a polyline of all raw input positions.  On each new input:
//! 1. Append to polyline
//! 2. Run N iterations of Laplacian smoothing on interior points (first + last pinned)
//! 3. Each iteration: `point[i] = lerp(point[i], avg(point[i-1], point[i+1]), strength)`
//! 4. Sensor values (pressure, tilt, etc.) smoothed the same way
//! 5. Diff against previous frame's polyline → find divergence point
//!
//! The tip is always pinned at the cursor (zero lag).  The stroke behind
//! the pen continuously reshapes as direction changes — the "taffy" feel.

use crate::brush::paint_info::PaintInformation;
use crate::brush::stabilizer::{StabilizerAlgorithm, StabilizerRegistration, StabilizeResult};
use crate::gpu::params::{ParamDef, ParamValue};

/// Threshold in pixels below which a point is considered unchanged.
const DIVERGENCE_EPSILON: f32 = 0.5;

const PARAMS: &[ParamDef] = &[
    ParamDef::Float { name: "strength", min: 0.0, max: 1.0, default: 0.5 },
];

pub fn register() -> StabilizerRegistration {
    StabilizerRegistration {
        type_id: "laplacian",
        display_name: "Laplacian Relaxation",
        params: PARAMS,
        from_params: |params| {
            let strength = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.5,
            };
            Box::new(LaplacianStabilizer::new(strength))
        },
    }
}

pub struct LaplacianStabilizer {
    raw_points: Vec<PaintInformation>,
    stabilized: Vec<PaintInformation>,
    prev_positions: Vec<[f32; 2]>,
    strength: f32,
}

impl LaplacianStabilizer {
    pub fn new(strength: f32) -> Self {
        Self {
            raw_points: Vec::with_capacity(256),
            stabilized: Vec::with_capacity(256),
            prev_positions: Vec::with_capacity(256),
            strength: strength.clamp(0.0, 1.0),
        }
    }

    /// Run Laplacian relaxation on the stabilized polyline.
    /// First and last points are pinned (never move).
    fn relax(&mut self) {
        let len = self.stabilized.len();
        if len < 3 {
            return;
        }

        let iterations = (self.strength * 5.0).ceil() as u32;
        let s = self.strength;

        for _ in 0..iterations {
            for i in 1..len - 1 {
                // Position smoothing.
                let prev_pos = self.stabilized[i - 1].pos;
                let next_pos = self.stabilized[i + 1].pos;
                let avg = [(prev_pos[0] + next_pos[0]) * 0.5, (prev_pos[1] + next_pos[1]) * 0.5];
                let cur = &mut self.stabilized[i];
                cur.pos[0] += (avg[0] - cur.pos[0]) * s;
                cur.pos[1] += (avg[1] - cur.pos[1]) * s;

                // Sensor smoothing — same treatment for all continuous values.
                let prev = self.stabilized[i - 1];
                let next = self.stabilized[i + 1];
                let cur = &mut self.stabilized[i];

                macro_rules! smooth_field {
                    ($field:ident) => {
                        let avg = (prev.$field + next.$field) * 0.5;
                        cur.$field += (avg - cur.$field) * s;
                    };
                }

                smooth_field!(pressure);
                smooth_field!(x_tilt);
                smooth_field!(y_tilt);
                smooth_field!(rotation);
                smooth_field!(tangential_pressure);
                smooth_field!(speed);
                smooth_field!(tilt_magnitude);
                smooth_field!(tilt_direction);
            }
        }
    }

    /// Find the divergence point: walk backward from tip until
    /// the position delta between current and previous frame < epsilon.
    fn find_divergence(&self) -> Option<usize> {
        let len = self.stabilized.len();
        if self.prev_positions.len() < len {
            // New points were added — at minimum, the new points diverge.
            // But we also need to check existing points that may have shifted.
            // Walk backward from the last point that existed in prev_positions.
            let check_from = self.prev_positions.len().saturating_sub(1);
            for i in (0..check_from).rev() {
                let cur = self.stabilized[i].pos;
                let prev = self.prev_positions[i];
                let dx = cur[0] - prev[0];
                let dy = cur[1] - prev[1];
                if dx * dx + dy * dy < DIVERGENCE_EPSILON * DIVERGENCE_EPSILON {
                    return Some(i + 1);
                }
            }
            return Some(0);
        }

        // Same length — walk backward from tip.
        for i in (0..len).rev() {
            let cur = self.stabilized[i].pos;
            let prev = self.prev_positions[i];
            let dx = cur[0] - prev[0];
            let dy = cur[1] - prev[1];
            if dx * dx + dy * dy < DIVERGENCE_EPSILON * DIVERGENCE_EPSILON {
                return if i + 1 < len { Some(i + 1) } else { None };
            }
        }
        Some(0)
    }
}

impl StabilizerAlgorithm for LaplacianStabilizer {
    fn push(&mut self, point: PaintInformation) -> StabilizeResult {
        // Save previous positions for divergence detection.
        self.prev_positions.clear();
        self.prev_positions.extend(self.stabilized.iter().map(|p| p.pos));

        // Append raw point.
        self.raw_points.push(point);

        // Copy raw → stabilized (fresh copy each frame for correct relaxation).
        self.stabilized.clear();
        self.stabilized.extend_from_slice(&self.raw_points);

        // Run relaxation.
        self.relax();

        // Find divergence.
        let divergence_index = if self.strength == 0.0 {
            None
        } else {
            self.find_divergence()
        };

        StabilizeResult { divergence_index }
    }

    fn stabilized(&self) -> &[PaintInformation] {
        &self.stabilized
    }

    fn max_divergence_window(&self) -> usize {
        if self.strength == 0.0 { return 0; }
        let iterations = (self.strength * 5.0).ceil() as usize;
        iterations * 10 + 5
    }

    fn clear(&mut self) {
        self.raw_points.clear();
        self.stabilized.clear();
        self.prev_positions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_point(x: f32, y: f32) -> PaintInformation {
        PaintInformation {
            pos: [x, y],
            pressure: 0.5,
            ..Default::default()
        }
    }

    fn make_point_with_pressure(x: f32, y: f32, pressure: f32) -> PaintInformation {
        PaintInformation {
            pos: [x, y],
            pressure,
            ..Default::default()
        }
    }

    #[test]
    fn straight_line_stays_straight() {
        let mut stab = LaplacianStabilizer::new(0.8);
        for i in 0..10 {
            stab.push(make_point(i as f32 * 10.0, 0.0));
        }

        // All points should be on y=0 (straight line is already smooth).
        for pt in stab.stabilized() {
            assert!(pt.pos[1].abs() < 1e-3, "y={} should be ~0", pt.pos[1]);
        }
    }

    #[test]
    fn sharp_turn_is_smoothed() {
        let mut stab = LaplacianStabilizer::new(0.8);

        // Straight right, then sharp turn down.
        for i in 0..5 {
            stab.push(make_point(i as f32 * 10.0, 0.0));
        }
        for i in 1..5 {
            stab.push(make_point(40.0, i as f32 * 10.0));
        }

        // The corner point (40, 0) should be pulled inward by smoothing.
        let corner = &stab.stabilized()[4];
        // It should have moved — either x decreased or y increased.
        let moved = corner.pos[0] < 40.0 - 0.1 || corner.pos[1] > 0.1;
        assert!(moved, "corner at {:?} should be smoothed away from (40, 0)", corner.pos);
    }

    #[test]
    fn strength_zero_is_pass_through() {
        let mut stab = LaplacianStabilizer::new(0.0);
        let points: Vec<_> = (0..5).map(|i| make_point(i as f32 * 10.0, (i as f32).sin() * 5.0)).collect();

        for pt in &points {
            let result = stab.push(*pt);
            assert!(result.divergence_index.is_none());
        }

        // Output should exactly match input.
        for (orig, stab_pt) in points.iter().zip(stab.stabilized()) {
            assert!((orig.pos[0] - stab_pt.pos[0]).abs() < 1e-6);
            assert!((orig.pos[1] - stab_pt.pos[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn first_and_last_pinned() {
        let mut stab = LaplacianStabilizer::new(1.0);

        // Zigzag pattern.
        stab.push(make_point(0.0, 0.0));
        stab.push(make_point(10.0, 20.0));
        stab.push(make_point(20.0, -20.0));
        stab.push(make_point(30.0, 20.0));
        stab.push(make_point(40.0, 0.0));

        let s = stab.stabilized();
        assert!((s[0].pos[0] - 0.0).abs() < 1e-6, "first point must be pinned");
        assert!((s[0].pos[1] - 0.0).abs() < 1e-6, "first point must be pinned");
        let last = s.last().unwrap();
        assert!((last.pos[0] - 40.0).abs() < 1e-6, "last point must be pinned");
        assert!((last.pos[1] - 0.0).abs() < 1e-6, "last point must be pinned");
    }

    #[test]
    fn divergence_detected_near_turn() {
        let mut stab = LaplacianStabilizer::new(0.5);

        // Build a straight stroke.
        for i in 0..10 {
            stab.push(make_point(i as f32 * 10.0, 0.0));
        }

        // Add a sharp turn — this should cause divergence near the end, not at the beginning.
        let result = stab.push(make_point(90.0, 30.0));
        if let Some(div) = result.divergence_index {
            assert!(div > 2, "divergence at {div} should be near the turn, not at the start");
        }
    }

    #[test]
    fn sensor_values_smoothed() {
        let mut stab = LaplacianStabilizer::new(0.8);

        // Pressure spike in the middle.
        stab.push(make_point_with_pressure(0.0, 0.0, 0.3));
        stab.push(make_point_with_pressure(10.0, 0.0, 0.3));
        stab.push(make_point_with_pressure(20.0, 0.0, 1.0));  // spike
        stab.push(make_point_with_pressure(30.0, 0.0, 0.3));
        stab.push(make_point_with_pressure(40.0, 0.0, 0.3));

        let s = stab.stabilized();
        // The spike should be smoothed down.
        assert!(s[2].pressure < 0.95, "pressure spike at {} should be smoothed", s[2].pressure);
        // Neighbors should be pulled up slightly.
        assert!(s[1].pressure > 0.3, "pressure {} should be pulled toward spike", s[1].pressure);
        assert!(s[3].pressure > 0.3, "pressure {} should be pulled toward spike", s[3].pressure);
    }

    #[test]
    fn higher_strength_smooths_more() {
        fn corner_displacement(strength: f32) -> f32 {
            let mut stab = LaplacianStabilizer::new(strength);
            for i in 0..5 {
                stab.push(make_point(i as f32 * 10.0, 0.0));
            }
            for i in 1..5 {
                stab.push(make_point(40.0, i as f32 * 10.0));
            }
            let corner = &stab.stabilized()[4];
            let dx = corner.pos[0] - 40.0;
            let dy = corner.pos[1] - 0.0;
            (dx * dx + dy * dy).sqrt()
        }

        let low = corner_displacement(0.2);
        let high = corner_displacement(0.8);
        assert!(high > low, "higher strength ({high}) should displace corner more than lower ({low})");
    }
}
