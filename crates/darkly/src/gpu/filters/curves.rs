//! Curves filter — per-channel tone mapping, modeled on Krita's "Color
//! Adjustment Curves" (`plugins/filters/colorsfilters`, `KisMultiChannelFilter`).
//!
//! For an RGBA image Krita 5.1+ exposes eight virtual channels, in this order
//! (`kis_multichannel_utils.cpp:59-64`, `virtual_channel_info.cpp`):
//!
//!   RGB (composite), Red, Green, Blue, Alpha, Hue, Saturation, Lightness
//!
//! Each is an independent curve edited on a 2D control-point editor (a
//! [`ParamValue::Curve`]), evaluated through the shared
//! [`CurveLut`](crate::brush::curve_math::CurveLut) natural-cubic spline (the
//! same Krita algorithm the brush curve node uses).
//!
//! Curves is a thin provider over the shared [`lut_param_filter`] scaffold: it
//! hands [`bake_lut`] eight per-channel spline evaluators and the shared code owns the
//! rest — the composite-over-channel fold, the HSV/Lab round trips gated by
//! `_active` flags, and the whole GPU pipeline (see
//! [`gpu::lut_filter`](crate::gpu::lut_filter) and `curves.wgsl`). Levels reuses
//! the identical scaffold with a different evaluator.

use std::sync::Arc;

use crate::brush::curve_math::CurveLut;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::lut_filter::{bake_lut, lut_param_filter, lut_shader_source, Baked};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::preview::{swing_signed, PreviewAnim};

/// Identity curve — a straight line through the two endpoints.
const IDENTITY: &[[f32; 2]] = &[[0.0, 0.0], [1.0, 1.0]];

/// Parameter schema — Krita's channel order for an RGBA image. Load-bearing:
/// [`build_lut`] indexes these positionally (matching [`Channel`]) and the
/// shader reads baked components in the same order.
pub const PARAMS: &[ParamDef] = &[
    ParamDef::curve("rgb", IDENTITY)
        .with_label("RGB")
        .with_description("Tone curve applied to all three colour channels together."),
    ParamDef::curve("red", IDENTITY)
        .with_label("Red")
        .with_description("Tone curve applied to the red channel alone."),
    ParamDef::curve("green", IDENTITY)
        .with_label("Green")
        .with_description("Tone curve applied to the green channel alone."),
    ParamDef::curve("blue", IDENTITY)
        .with_label("Blue")
        .with_description("Tone curve applied to the blue channel alone."),
    ParamDef::curve("alpha", IDENTITY)
        .with_label("Alpha")
        .with_description("Tone curve applied to opacity."),
    ParamDef::curve("hue", IDENTITY)
        .with_label("Hue")
        .with_description("Remaps hue against itself, shifting colours around the wheel."),
    ParamDef::curve("saturation", IDENTITY)
        .with_label("Saturation")
        .with_description("Remaps saturation, letting muted and vivid areas move apart."),
    ParamDef::curve("lightness", IDENTITY)
        .with_label("Lightness")
        .with_description("Remaps lightness independently of hue and saturation."),
];

/// Read a curve param's control points by index, falling back to identity when
/// the param is missing or malformed (fewer than two points).
fn curve_points(params: &[ParamValue], idx: usize) -> Vec<[f32; 2]> {
    match params.get(idx) {
        Some(ParamValue::Curve(pts)) if pts.len() >= 2 => pts.clone(),
        _ => IDENTITY.to_vec(),
    }
}

/// Bake the eight tone curves into the shared LUT + gate flags. The channel
/// semantics (composite fold, HSL rows, gating) live in [`bake_lut`]; here we
/// just supply the eight spline evaluators.
fn build_lut(params: &[ParamValue]) -> Baked {
    let luts: Vec<CurveLut> = (0..PARAMS.len())
        .map(|i| CurveLut::from_points(&curve_points(params, i)))
        .collect();
    bake_lut(|ch, t| luts[ch as usize].evaluate(t))
}

fn create_pipeline(device: &wgpu::Device) -> Arc<dyn FilterEffect> {
    Arc::new(lut_param_filter(device, &lut_shader_source(), build_lut))
}

/// Contrast rises above the identity transfer, falls below it, and returns.
///
/// The swept curve carries four control points at fixed `x` coordinates, so
/// only `y` moves and the points stay ascending for [`CurveLut::from_points`]'
/// sorted-input precondition. `IDENTITY` is deliberately not the resting curve:
/// it holds two points, an S-curve needs four, and a preview that changed point
/// count mid-sweep would step rather than move. The four-point resting curve is
/// collinear, so the natural-cubic spline reproduces the straight line exactly
/// and the resting frames match a default-parameter render.
///
/// Only the composite `rgb` channel moves; the other seven stay at their
/// two-point defaults, which keeps the per-channel and HSL rows of the baked LUT
/// inactive and the result reading as a tone swing rather than a hue mess.
fn preview_params(t: f32) -> Vec<ParamValue> {
    let bend = 0.18 * swing_signed(t);
    let mut params: Vec<ParamValue> = PARAMS.iter().map(ParamDef::default_value).collect();
    params[0] = ParamValue::Curve(vec![
        // rgb
        [0.0, 0.0],
        [0.25, 0.25 - bend],
        [0.75, 0.75 + bend],
        [1.0, 1.0],
    ]);
    params
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "curves",
        display_name: "Curves",
        icon: "fa6-solid:chart-line",
        description: "Remap tones and colors with editable per-channel curves.",
        params: PARAMS,
        // A signed sweep rests in the middle, so the default still would be the
        // frame that looks like no effect at all. The quarter point is its
        // positive extreme.
        preview: Some(PreviewAnim::LOOPING.with_still_at(0.25)),
        preview_at: Some(preview_params),
        create_pipeline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::lut_filter::{Channel, LUT_BYTES, LUT_LEN};

    fn curve(points: &[[f32; 2]]) -> ParamValue {
        ParamValue::Curve(points.to_vec())
    }

    /// Row 0, component `c` (0=r,1=g,2=b,3=a) at input index `i`.
    fn row0(lut: &[u8; LUT_BYTES], i: usize, c: usize) -> u8 {
        lut[i * 4 + c]
    }
    /// Row 1, component `c` (0=hue,1=sat,2=lightness) at input index `i`.
    fn row1(lut: &[u8; LUT_BYTES], i: usize, c: usize) -> u8 {
        lut[LUT_LEN * 4 + i * 4 + c]
    }

    /// Positional indices into [`PARAMS`] — mirror [`Channel`] for readable tests.
    const RGB: usize = Channel::Rgb as usize;
    const RED: usize = Channel::Red as usize;
    const SATURATION: usize = Channel::Saturation as usize;
    const LIGHTNESS: usize = Channel::Lightness as usize;

    /// Identity curves ⇒ identity LUT on every channel (both rows), and neither
    /// gated stage is active. This is the invariant the `textureLoad(round(v*255))`
    /// index convention relies on for a bit-exact no-op.
    #[test]
    fn identity_curves_yield_identity_lut() {
        let params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        let baked = build_lut(&params);
        for i in 0..LUT_LEN {
            for c in 0..4 {
                assert_eq!(
                    row0(&baked.lut, i, c) as usize,
                    i,
                    "row0 entry {i} chan {c}"
                );
            }
            for c in 0..3 {
                assert_eq!(
                    row1(&baked.lut, i, c) as usize,
                    i,
                    "row1 entry {i} chan {c}"
                );
            }
        }
        assert!(
            !baked.hsv_active,
            "identity HSL curves must not arm the HSV pass"
        );
        assert!(
            !baked.lightness_active,
            "identity lightness must not arm the Lab pass"
        );
    }

    /// Fold order matches Krita: the color channels are `rgb(channel(i))` — the
    /// per-channel curve first, then the composite "RGB" curve on top. Both
    /// curves are two-point (exact linear in `CurveLut`) so the arithmetic is
    /// unambiguous.
    #[test]
    fn composite_curve_composes_over_channel_curve() {
        // red(t) = 0.5·t (halve); rgb(x) = min(2·x, 1) (double, clamped).
        // Correct fold rgb(red(t)) = min(t, 1) = t → identity.
        // Wrong fold red(rgb(t)) = 0.5·min(2t,1) would map the top to 0.5.
        let mut params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        params[RGB] = curve(&[[0.0, 0.0], [0.5, 1.0]]); // composite doubles
        params[RED] = curve(&[[0.0, 0.0], [1.0, 0.5]]); // red halves
        let baked = build_lut(&params);
        // rgb(red(1.0)) = min(2·0.5, 1) = 1.0 → 255. Wrong order gives 128.
        assert_eq!(
            row0(&baked.lut, 255, 0),
            255,
            "rgb(red(255)) must map to 255"
        );
        // Midpoint: rgb(red(0.5)) = min(2·0.25, 1) = 0.5 → ~128.
        assert!(
            (row0(&baked.lut, 128, 0) as i32 - 128).abs() <= 2,
            "rgb(red(0.5)) ≈ 0.5, got {}",
            row0(&baked.lut, 128, 0)
        );
    }

    /// The composite "RGB" curve is applied to R/G/B but never to alpha (Krita
    /// `:241`): with a non-identity composite curve and identity alpha, the
    /// alpha column stays identity.
    #[test]
    fn composite_curve_never_touches_alpha() {
        let mut params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        params[RGB] = curve(&[[0.0, 0.0], [0.5, 1.0]]); // aggressive composite
        let baked = build_lut(&params);
        for i in 0..LUT_LEN {
            assert_eq!(
                row0(&baked.lut, i, 3) as usize,
                i,
                "alpha must stay identity regardless of the composite curve, entry {i}"
            );
        }
    }

    /// A non-identity Hue or Saturation curve arms the HSV pass; a non-identity
    /// Lightness curve arms the Lab pass — independently.
    #[test]
    fn hsl_curves_arm_their_stages() {
        let mut p = PARAMS.iter().map(|d| d.default_value()).collect::<Vec<_>>();
        p[SATURATION] = curve(&[[0.0, 0.0], [1.0, 0.5]]);
        let baked = build_lut(&p);
        assert!(baked.hsv_active, "a saturation curve must arm the HSV pass");
        assert!(!baked.lightness_active);

        let mut p = PARAMS.iter().map(|d| d.default_value()).collect::<Vec<_>>();
        p[LIGHTNESS] = curve(&[[0.0, 0.0], [1.0, 0.5]]);
        let baked = build_lut(&p);
        assert!(
            baked.lightness_active,
            "a lightness curve must arm the Lab pass"
        );
        assert!(!baked.hsv_active);
    }
}
