//! Parametric vector shapes: the families a shape object can be drawn from.
//!
//! A shape is stored as a family id plus its schema-ordered parameter values
//! and a local box size ([`crate::layer::ShapeProps`]); the path is derived
//! here at scene-build time and never stored. Each family is one file under
//! `families/` exporting `pub fn register()`, discovered by `build.rs`, and is
//! drawn by a tool of its own (`tools/shape.rs`).

pub mod families;
pub mod polar;

use std::sync::OnceLock;

use kurbo::{Affine, BezPath, Point, Rect, Shape};

use crate::gpu::params::{ParamDef, ParamValue};
use crate::layer::ShapeProps;

/// Flattening tolerance for arcs and ellipses, in object-local pixels.
pub const TOLERANCE: f64 = 0.1;

/// One shape family: its identity, its parameter schema, and its generator.
pub struct ShapeFamilyRegistration {
    /// Stable id stored in [`ShapeProps::family`].
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Glyph of the family. It heads the family's catalog entry and is the
    /// icon of the tool that draws it (`tools/shape.rs` projects every family
    /// into one tool).
    pub icon: &'static str,
    /// Id of the action that selects this family's tool; presets bind it.
    pub hotkey_action: &'static str,
    pub params: &'static [ParamDef],
    /// Build the closed outline in the local box `[0, w] x [0, h]`. The
    /// result's bounding box must equal that box: hit testing and the gizmo
    /// frame read the box, never the path (pinned by
    /// `every_family_outline_is_closed_and_box_tight`).
    pub outline: fn(params: &[ParamValue], w: f64, h: f64) -> BezPath,
}

/// Id of the catalog this registry projects into.
pub const CATALOG_ID: &str = "shapes";

/// Every registered family, sorted by `type_id`.
pub fn registry() -> &'static [ShapeFamilyRegistration] {
    static REGISTRY: OnceLock<Vec<ShapeFamilyRegistration>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut regs = families::registrations();
        regs.sort_by_key(|r| r.type_id);
        regs
    })
}

pub fn find(type_id: &str) -> Option<&'static ShapeFamilyRegistration> {
    registry().iter().find(|r| r.type_id == type_id)
}

/// The shape catalog: every family with its parameter schema and defaults.
pub fn catalog() -> crate::catalog::Catalog {
    crate::catalog::Catalog::new(
        CATALOG_ID,
        "Shapes",
        registry()
            .iter()
            .map(|r| {
                crate::catalog::CatalogEntry::new(r.type_id, r.display_name)
                    .with_description(r.description)
                    .with_icon(r.icon)
                    .with_params(r.params)
            })
            .collect(),
    )
    .with_description("Parametric outlines the shape tool draws onto a vector layer.")
}

/// The shape's outline in its local box. Empty for an unknown family, which
/// draws nothing rather than failing the whole layer.
pub fn tessellate(props: &ShapeProps) -> BezPath {
    match find(&props.family) {
        Some(reg) => (reg.outline)(&props.params, props.width as f64, props.height as f64),
        None => BezPath::new(),
    }
}

/// The affine taking `from` onto `to` with an independent scale per axis. An
/// axis on which `from` is degenerate maps onto the centre line of `to`.
pub fn box_fit(from: Rect, to: Rect) -> Affine {
    let axis = |lo: f64, len: f64, to_lo: f64, to_len: f64| {
        if len > 1e-12 {
            let s = to_len / len;
            (s, to_lo - lo * s)
        } else {
            (0.0, to_lo + to_len * 0.5)
        }
    };
    let (sx, tx) = axis(from.x0, from.width(), to.x0, to.width());
    let (sy, ty) = axis(from.y0, from.height(), to.y0, to.height());
    Affine::new([sx, 0.0, 0.0, sy, tx, ty])
}

/// Map `path` so its exact bounding box is `[0, w] x [0, h]`.
pub fn fit_to_box(path: BezPath, w: f64, h: f64) -> BezPath {
    let fit = box_fit(path.bounding_box(), Rect::new(0.0, 0.0, w, h));
    fit * path
}

/// Closed uniform Catmull-Rom spline through cyclic samples, one cubic per
/// segment. Cubics rather than a polyline so a large gizmo scale stays smooth:
/// Vello flattens at device resolution. (Catmull and Rom, 1974.)
pub fn closed_catmull_rom(points: &[Point]) -> BezPath {
    let n = points.len();
    let mut path = BezPath::new();
    if n == 0 {
        return path;
    }
    let p = |i: usize| points[i % n].to_vec2();
    path.move_to(points[0]);
    for i in 0..n {
        let (p0, p1, p2, p3) = (p(i + n - 1), p(i), p(i + 1), p(i + 2));
        path.curve_to(
            (p1 + (p2 - p0) / 6.0).to_point(),
            (p2 - (p3 - p1) / 6.0).to_point(),
            p2.to_point(),
        );
    }
    path.close_path();
    path
}

/// A polar curve `r(θ)` sampled `n` times, splined, and fitted to the box.
pub fn polar_outline(n: usize, r: impl Fn(f64) -> f64, w: f64, h: f64) -> BezPath {
    fit_to_box(closed_catmull_rom(&polar::sample_polar(n, r)), w, h)
}

/// Sample count for a polar curve: `per_unit` samples per unit of `detail`,
/// clamped to `[lo, hi]`. [`polar::sample_polar`] rounds to a multiple of 4.
pub fn sample_count(detail: f64, per_unit: f64, lo: usize, hi: usize) -> usize {
    ((detail * per_unit).ceil() as usize).clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::params::ParamKind;
    use kurbo::PathEl;

    /// Every way to draw each family: its defaults, then once per option of
    /// each `Enum` parameter with the rest at default. A family that grows a
    /// mode is covered without naming it here.
    fn every_variant(reg: &ShapeFamilyRegistration) -> Vec<(String, Vec<ParamValue>)> {
        let defaults: Vec<ParamValue> = reg.params.iter().map(ParamDef::default_value).collect();
        let mut out = vec![("defaults".to_string(), defaults.clone())];
        for (slot, def) in reg.params.iter().enumerate() {
            if let ParamKind::Enum { options, .. } = def.kind {
                for (i, label) in options.iter().enumerate() {
                    let mut p = defaults.clone();
                    p[slot] = ParamValue::Int(i as i32);
                    out.push((format!("{} = {label}", def.name), p));
                }
            }
        }
        out
    }

    #[test]
    fn every_family_outline_is_closed_and_box_tight() {
        assert!(!registry().is_empty());
        for reg in registry() {
            for (variant, params) in every_variant(reg) {
                let what = format!("`{}` ({variant})", reg.type_id);
                let path = (reg.outline)(&params, 200.0, 100.0);
                let els = path.elements();
                assert_eq!(
                    els.iter()
                        .filter(|e| matches!(e, PathEl::MoveTo(_)))
                        .count(),
                    1,
                    "{what} must be a single subpath"
                );
                assert!(
                    matches!(els.last(), Some(PathEl::ClosePath)),
                    "{what} is not closed"
                );
                let b = path.bounding_box();
                for (got, want) in [(b.x0, 0.0), (b.y0, 0.0), (b.x1, 200.0), (b.y1, 100.0)] {
                    assert!((got - want).abs() < 0.5, "{what} bbox {b:?} is not the box");
                }
            }
        }
    }

    /// The `visible_when` contract: a parameter hidden under the current
    /// value of its selector does not change the outline.
    #[test]
    fn a_hidden_parameter_does_not_change_the_outline() {
        for reg in registry() {
            for (variant, params) in every_variant(reg) {
                let base = (reg.outline)(&params, 120.0, 80.0);
                for (slot, def) in reg.params.iter().enumerate() {
                    let Some((selector, allowed)) = def.visible_when else {
                        continue;
                    };
                    let sel = reg.params.iter().position(|d| d.name == selector).unwrap();
                    let current = match params[sel] {
                        ParamValue::Int(v) => v,
                        _ => continue,
                    };
                    if allowed.contains(&current) {
                        continue;
                    }
                    let mut nudged = params.clone();
                    nudged[slot] = match (&def.kind, &params[slot]) {
                        (ParamKind::Float { max, .. }, _) => ParamValue::Float(*max),
                        (ParamKind::Int { max, .. }, _) => ParamValue::Int(*max),
                        (_, v) => v.clone(),
                    };
                    assert_eq!(
                        base.elements(),
                        (reg.outline)(&nudged, 120.0, 80.0).elements(),
                        "`{}` ({variant}): hidden `{}` changed the outline",
                        reg.type_id,
                        def.name
                    );
                }
            }
        }
    }

    #[test]
    fn box_fit_centres_a_degenerate_axis() {
        let fit = box_fit(
            Rect::new(1.0, 5.0, 3.0, 5.0),
            Rect::new(0.0, 0.0, 10.0, 20.0),
        );
        assert_eq!(fit * Point::new(1.0, 5.0), Point::new(0.0, 10.0));
        assert_eq!(fit * Point::new(3.0, 5.0), Point::new(10.0, 10.0));
    }

    #[test]
    fn catmull_rom_passes_through_its_samples() {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(0.0, 1.0),
        ];
        let path = closed_catmull_rom(&pts);
        let ends: Vec<Point> = path
            .elements()
            .iter()
            .filter_map(|e| match e {
                PathEl::CurveTo(_, _, p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(ends, [pts[1], pts[2], pts[3], pts[0]]);
    }
}
