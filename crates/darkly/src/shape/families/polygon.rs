//! Regular polygon with optional circular corners, fitted to the box.
//!
//! The rounding is a Minkowski sum with a disc: the sharp polygon is fitted
//! into the box inset by the corner radius, then every edge is pushed out by
//! the radius and every corner becomes an arc about its vertex. The offset
//! grows the bounding box by exactly the radius on each side, so the result
//! fills the box, and applying it after the (possibly non-uniform) fit keeps
//! the fillets circular. This is the path form of the brush `polygon` node's
//! "build at circumradius `1 - ρ`, then `sd - ρ`".

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use kurbo::{Arc, BezPath, Point, Rect, Vec2};

use crate::gpu::params::{ParamDef, ParamSlots, ParamValue};
use crate::shape::{box_fit, ShapeFamilyRegistration, TOLERANCE};
use crate::units::UnitType;

const PARAMS: &[ParamDef] = &[
    ParamDef::int("points", 3, 32, 5)
        .with_label("Sides")
        .with_description("Number of sides (min 3)."),
    ParamDef::float("rounding", 0.0, 1.0, 0.0)
        .with_label("Rounding")
        .with_unit(UnitType::Percent)
        .with_description("Round the corners: 0% = sharp, 100% = circle."),
];

pub fn register() -> ShapeFamilyRegistration {
    ShapeFamilyRegistration {
        type_id: "polygon",
        display_name: "Polygon",
        description: "A regular polygon resting on a flat base, with optional rounded corners, filling the dragged box.",
        icon: "tabler:pentagon-filled",
        hotkey_action: "polygonTool",
        params: PARAMS,
        outline,
    }
}

fn outline(params: &[ParamValue], w: f64, h: f64) -> BezPath {
    let n = params.int_at(0, 5).max(3) as usize;
    let rounding = (params.float_at(1, 0.0) as f64).clamp(0.0, 1.0);
    // Shy of the full half-side so the inner polygon never collapses to a
    // point; at 100% the result is visually a circle or stadium.
    let half = w.min(h) * 0.5;
    let r = (rounding * half).min(half * (1.0 - 1e-3));

    // Symmetric about straight down (y-down), so one edge rests flat on the
    // base: a square is axis-aligned and a triangle points up.
    let base: Vec<Point> = (0..n)
        .map(|i| {
            let a = FRAC_PI_2 + PI / n as f64 + TAU * i as f64 / n as f64;
            Point::new(a.cos(), a.sin())
        })
        .collect();
    let bounds = base
        .iter()
        .fold(Rect::from_points(base[0], base[0]), |b, p| b.union_pt(*p));
    let fit = box_fit(bounds, Rect::new(r, r, w - r, h - r));
    let v: Vec<Point> = base.iter().map(|p| fit * *p).collect();

    let mut path = BezPath::new();
    if r <= 0.0 {
        path.move_to(v[0]);
        for p in &v[1..] {
            path.line_to(*p);
        }
        path.close_path();
        return path;
    }

    // Outward unit normal of edge `v[i] -> v[i + 1]`.
    let centroid = v.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / n as f64;
    let normal = |i: usize| {
        let (a, b) = (v[i], v[(i + 1) % n]);
        let d = (b - a).normalize();
        let nrm = Vec2::new(d.y, -d.x);
        if nrm.dot(a.to_vec2() - centroid) < 0.0 {
            -nrm
        } else {
            nrm
        }
    };
    path.move_to(v[0] + normal(n - 1) * r);
    for i in 0..n {
        let (from, to) = (normal((i + n - 1) % n), normal(i));
        let start = from.atan2();
        let sweep = (to.atan2() - start + PI).rem_euclid(TAU) - PI;
        Arc::new(v[i], (r, r), start, sweep, 0.0)
            .append_iter(TOLERANCE)
            .for_each(|el| path.push(el));
        path.line_to(v[(i + 1) % n] + to * r);
    }
    path.close_path();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn p(points: i32, rounding: f32) -> [ParamValue; 2] {
        [ParamValue::Int(points), ParamValue::Float(rounding)]
    }

    #[test]
    fn sharp_square_fills_the_box() {
        let area = outline(&p(4, 0.0), 200.0, 100.0).area().abs();
        assert!((area - 20_000.0).abs() < 1e-6, "area {area}");
    }

    #[test]
    fn triangle_points_up() {
        let path = outline(&p(3, 0.0), 100.0, 100.0);
        let apex = path
            .elements()
            .iter()
            .filter_map(|e| e.end_point())
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .unwrap();
        assert!(
            (apex.x - 50.0).abs() < 1e-9 && apex.y.abs() < 1e-9,
            "{apex:?}"
        );
    }

    #[test]
    fn full_rounding_on_a_square_is_a_disc() {
        let area = outline(&p(4, 1.0), 100.0, 100.0).area().abs();
        let want = PI * 50.0 * 50.0;
        assert!((area - want).abs() / want < 0.01, "area {area}");
    }

    #[test]
    fn rounded_hexagon_is_box_tight() {
        let b = outline(&p(6, 0.4), 160.0, 90.0).bounding_box();
        let tol = TOLERANCE + 0.01;
        assert!(b.x0.abs() < tol && b.y0.abs() < tol, "{b:?}");
        assert!(
            (b.x1 - 160.0).abs() < tol && (b.y1 - 90.0).abs() < tol,
            "{b:?}"
        );
    }
}
