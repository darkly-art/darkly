//! Ellipse family: a polar curve `r(θ)` fitted to the box, in one of the brush
//! `circle` node's three silhouettes. Smooth is the sine harmonic
//! `r(θ) = 1 + A·sin(f·θ)` (at zero ripple the exact ellipse), Blob the
//! periodic Perlin fBm `r(θ) = 1 + A·(2·fbm − 1)`, and Superformula Gielis'
//! curve. Each style keeps its own knobs; the others are hidden while it is
//! drawn and keep their values.

use kurbo::{BezPath, Ellipse, Shape};

use crate::gpu::params::{ParamDef, ParamSlots, ParamValue};
use crate::shape::{polar, polar_outline, sample_count, ShapeFamilyRegistration, TOLERANCE};
use crate::units::UnitType;

const STYLE_SMOOTH: i32 = 0;
const STYLE_BLOB: i32 = 1;
const STYLE_SUPERFORMULA: i32 = 2;

// Slot of each parameter in `PARAMS`, so no outline reads a bare index.
const STYLE: usize = 0;
const RIPPLE: usize = 1;
const BUMPS: usize = 2;
const WOBBLE: usize = 3;
const LOBES: usize = 4;
const PERSISTENCE: usize = 5;
const OCTAVES: usize = 6;
const SEED: usize = 7;
const M: usize = 8;
const N1: usize = 9;
const N2: usize = 10;
const N3: usize = 11;

const PARAMS: &[ParamDef] = &[
    ParamDef::enumeration("style", &["Smooth", "Blob", "Superformula"], STYLE_SMOOTH)
        .with_label("Style")
        .with_description(
            "Smooth ellipses and flowers, organic blobs, or superformula stars, \
             flowers and rounded polygons.",
        ),
    ParamDef::float("ripple", 0.0, 0.9, 0.0)
        .with_label("Ripple")
        .with_unit(UnitType::Percent)
        .with_description("How far the edge swells and dips around the plain ellipse.")
        .with_visible_when("style", &[STYLE_SMOOTH]),
    ParamDef::int("bumps", 1, 32, 6)
        .with_label("Bumps")
        .with_description("Number of ripples around the edge.")
        .with_visible_when("style", &[STYLE_SMOOTH]),
    ParamDef::float("wobble", 0.0, 0.9, 0.35)
        .with_label("Wobble")
        .with_unit(UnitType::Percent)
        .with_description("How far the edge wanders in and out.")
        .with_visible_when("style", &[STYLE_BLOB]),
    ParamDef::int("lobes", 1, 16, 3)
        .with_label("Lobes")
        .with_description("Number of broad lobes around the edge.")
        .with_visible_when("style", &[STYLE_BLOB]),
    ParamDef::float("persistence", 0.0, 1.0, 0.5)
        .with_label("Roughness")
        .with_unit(UnitType::Percent)
        .with_description("How strongly the finer detail layers show through.")
        .with_visible_when("style", &[STYLE_BLOB]),
    ParamDef::int("octaves", 1, 6, 3)
        .with_label("Detail")
        .with_description("How many layers of ever-finer wobble are stacked up.")
        .with_visible_when("style", &[STYLE_BLOB]),
    ParamDef::int("seed", 0, 9999, 0)
        .with_label("Seed")
        .with_description("Picks which blob is drawn; any two seeds look unrelated.")
        .with_visible_when("style", &[STYLE_BLOB]),
    ParamDef::int("m", 0, 32, 5)
        .with_label("Symmetry")
        .with_description("How many times the pattern repeats around the centre.")
        .with_visible_when("style", &[STYLE_SUPERFORMULA]),
    ParamDef::float("n1", 0.1, 16.0, 2.0)
        .with_label("Fullness")
        .with_description("Low values pinch the shape into sharp spikes; high values puff it out.")
        .with_visible_when("style", &[STYLE_SUPERFORMULA]),
    ParamDef::float("n2", 0.1, 16.0, 7.0)
        .with_label("Rise")
        .with_description("Shape of each lobe's leading flank.")
        .with_visible_when("style", &[STYLE_SUPERFORMULA]),
    ParamDef::float("n3", 0.1, 16.0, 7.0)
        .with_label("Fall")
        .with_description("Shape of each lobe's trailing flank.")
        .with_visible_when("style", &[STYLE_SUPERFORMULA]),
];

pub fn register() -> ShapeFamilyRegistration {
    ShapeFamilyRegistration {
        type_id: "ellipse",
        display_name: "Ellipse",
        description: "An ellipse filling the dragged box: smooth or rippled, an organic blob, \
                      or a superformula star or flower.",
        icon: "tabler:oval-filled",
        hotkey_action: "ellipseTool",
        params: PARAMS,
        outline,
    }
}

fn outline(params: &[ParamValue], w: f64, h: f64) -> BezPath {
    match params.int_at(STYLE, STYLE_SMOOTH) {
        STYLE_BLOB => blob(params, w, h),
        STYLE_SUPERFORMULA => superformula(params, w, h),
        _ => smooth(params, w, h),
    }
}

fn smooth(params: &[ParamValue], w: f64, h: f64) -> BezPath {
    let amplitude = params.float_at(RIPPLE, 0.0) as f64;
    let frequency = params.int_at(BUMPS, 6).max(1) as f64;
    if amplitude <= 0.0 {
        // The plain case is drawn exactly rather than through samples. kurbo
        // leaves the ellipse open; closing it gives the border a proper join.
        let mut path = Ellipse::new((w * 0.5, h * 0.5), (w * 0.5, h * 0.5), 0.0).to_path(TOLERANCE);
        path.close_path();
        return path;
    }
    // Enough samples per ripple that the spline never shows its knots.
    let n = sample_count(frequency, 64.0, 256, 2048);
    polar_outline(n, |t| polar::r_sine(t, amplitude, frequency), w, h)
}

fn blob(params: &[ParamValue], w: f64, h: f64) -> BezPath {
    let amplitude = params.float_at(WOBBLE, 0.35) as f64;
    let frequency = params.int_at(LOBES, 3).max(1) as u32;
    let persistence = params.float_at(PERSISTENCE, 0.5) as f64;
    let octaves = params.int_at(OCTAVES, 3).clamp(1, 6) as u32;
    let seed = params.int_at(SEED, 0).max(0) as u32;
    // Eight samples per cell of the finest octave.
    let finest = frequency as f64 * (1u32 << (octaves - 1)) as f64;
    let n = sample_count(finest, 8.0, 256, 4096);
    let r = |t| polar::r_perlin(t, amplitude, frequency, persistence, seed, octaves);
    polar_outline(n, r, w, h)
}

fn superformula(params: &[ParamValue], w: f64, h: f64) -> BezPath {
    let m = params.int_at(M, 5).max(0) as f64;
    let n1 = (params.float_at(N1, 2.0) as f64).max(0.01);
    let n2 = params.float_at(N2, 7.0) as f64;
    let n3 = params.float_at(N3, 7.0) as f64;
    let n = sample_count(m, 64.0, 256, 2048);
    polar_outline(n, |t| polar::r_superformula(t, m, n1, n2, n3), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// The defaults with some slots overridden.
    fn params(overrides: &[(usize, ParamValue)]) -> Vec<ParamValue> {
        let mut p: Vec<ParamValue> = PARAMS.iter().map(ParamDef::default_value).collect();
        for (slot, v) in overrides {
            p[*slot] = v.clone();
        }
        p
    }

    #[test]
    fn plain_ellipse_is_exact() {
        let area = outline(&params(&[]), 200.0, 100.0).area();
        let want = PI * 100.0 * 50.0;
        assert!((area.abs() - want).abs() / want < 1e-3, "area {area}");
    }

    #[test]
    fn ripples_change_the_outline() {
        let plain = outline(&params(&[]), 100.0, 100.0).area();
        let rippled = outline(&params(&[(RIPPLE, ParamValue::Float(0.4))]), 100.0, 100.0).area();
        assert!((plain - rippled).abs() > 1.0);
    }

    fn blob_with_seed(seed: i32) -> Vec<ParamValue> {
        params(&[
            (STYLE, ParamValue::Int(STYLE_BLOB)),
            (SEED, ParamValue::Int(seed)),
        ])
    }

    #[test]
    fn seed_picks_the_blob() {
        let a = outline(&blob_with_seed(3), 100.0, 100.0);
        assert_eq!(
            a.elements(),
            outline(&blob_with_seed(3), 100.0, 100.0).elements()
        );
        assert_ne!(
            a.elements(),
            outline(&blob_with_seed(4), 100.0, 100.0).elements()
        );
    }

    #[test]
    fn blob_stays_inside_the_box() {
        let b = outline(&blob_with_seed(9), 120.0, 80.0).bounding_box();
        assert!(
            b.x0 > -0.5 && b.y0 > -0.5 && b.x1 < 120.5 && b.y1 < 80.5,
            "{b:?}"
        );
    }

    #[test]
    fn odd_superformula_symmetry_does_not_panic_and_stays_boxed() {
        let p = params(&[
            (STYLE, ParamValue::Int(STYLE_SUPERFORMULA)),
            (M, ParamValue::Int(1)),
            (N1, ParamValue::Float(0.3)),
            (N2, ParamValue::Float(16.0)),
            (N3, ParamValue::Float(0.1)),
        ]);
        let b = outline(&p, 50.0, 70.0).bounding_box();
        assert!((b.width() - 50.0).abs() < 1e-6 && (b.height() - 70.0).abs() < 1e-6);
    }

    #[test]
    fn slot_constants_name_their_params() {
        let named = [
            (STYLE, "style"),
            (RIPPLE, "ripple"),
            (BUMPS, "bumps"),
            (WOBBLE, "wobble"),
            (LOBES, "lobes"),
            (PERSISTENCE, "persistence"),
            (OCTAVES, "octaves"),
            (SEED, "seed"),
            (M, "m"),
            (N1, "n1"),
            (N2, "n2"),
            (N3, "n3"),
        ];
        assert_eq!(named.len(), PARAMS.len());
        for (slot, name) in named {
            assert_eq!(PARAMS[slot].name, name);
        }
    }
}
