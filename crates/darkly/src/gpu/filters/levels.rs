//! Levels filter — parametric black-point / gamma / white-point / output-range
//! tone mapping, modeled on Krita's `KisLevelsFilter`.
//!
//! A Levels adjustment is mathematically a parametric curve, so it shares the
//! entire GPU realization with [Curves](super::curves): the same eight virtual
//! channels (RGB composite, Red, Green, Blue, Alpha, Hue, Saturation, Lightness),
//! the same 256×2 LUT, the same `curves.wgsl` shader. Levels is a thin provider
//! over the shared [`lut_param_filter`] scaffold — it hands [`bake_lut`] eight
//! per-channel [`levels_transfer`] evaluators and the shared code owns the composite fold,
//! the HSV/Lab round trips, and the pipeline.
//!
//! Transfer function — Krita `libs/image/KisLevelsCurve.cpp:58`:
//!   `out = outBlack + (outWhite − outBlack) · clamp((x−inBlack)/(inWhite−inBlack), 0, 1)^(1/gamma)`
//! clamped to `outBlack` for `x ≤ inBlack` and `outWhite` for `x ≥ inWhite`.
//! Config order `[inBlack, inWhite, gamma, outBlack, outWhite]` matches
//! `KisLevelsFilterConfiguration.cpp`.

use std::sync::Arc;

use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::lut_filter::{bake_lut, lut_param_filter, lut_shader_source, Baked};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::preview::{swing_signed, PreviewAnim};

/// Identity levels — `[inBlack, inWhite, gamma, outBlack, outWhite]`. Maps the
/// full `[0,1]` input range linearly onto `[0,1]` output: a no-op transfer.
const IDENTITY: [f32; 5] = [0.0, 1.0, 1.0, 0.0, 1.0];

/// Parameter schema — Krita's channel order for an RGBA image, identical to
/// [Curves](super::curves::PARAMS). Load-bearing: [`build_lut`] indexes these
/// positionally (matching [`Channel`](crate::gpu::lut_filter::Channel)).
pub const PARAMS: &[ParamDef] = &[
    ParamDef::levels("rgb", IDENTITY)
        .with_label("RGB")
        .with_description(
            "Black point, white point and gamma for all three color channels together.",
        ),
    ParamDef::levels("red", IDENTITY)
        .with_label("Red")
        .with_description("Black point, white point and gamma for the red channel alone."),
    ParamDef::levels("green", IDENTITY)
        .with_label("Green")
        .with_description("Black point, white point and gamma for the green channel alone."),
    ParamDef::levels("blue", IDENTITY)
        .with_label("Blue")
        .with_description("Black point, white point and gamma for the blue channel alone."),
    ParamDef::levels("alpha", IDENTITY)
        .with_label("Alpha")
        .with_description("Black point, white point and gamma for opacity."),
    ParamDef::levels("hue", IDENTITY)
        .with_label("Hue")
        .with_description("Black point, white point and gamma applied to hue."),
    ParamDef::levels("saturation", IDENTITY)
        .with_label("Saturation")
        .with_description("Black point, white point and gamma applied to saturation."),
    ParamDef::levels("lightness", IDENTITY)
        .with_label("Lightness")
        .with_description("Black point, white point and gamma applied to lightness."),
];

/// Read a levels param by index, falling back to identity when missing/malformed.
fn levels_params(params: &[ParamValue], idx: usize) -> [f32; 5] {
    match params.get(idx) {
        Some(ParamValue::Levels(a)) => *a,
        _ => IDENTITY,
    }
}

/// Krita's `KisLevelsCurve` transfer (see module docs). `x` and the result are
/// normalized `[0,1]`; `gamma` is the raw exponent (`0.1–10`, `1.0` = linear).
fn levels_transfer(p: &[f32; 5], x: f32) -> f32 {
    let [in_black, in_white, gamma, out_black, out_white] = *p;
    if x <= in_black {
        return out_black;
    }
    if x >= in_white {
        return out_white;
    }
    let range = in_white - in_black;
    let norm = if range <= f32::EPSILON {
        0.0
    } else {
        ((x - in_black) / range).clamp(0.0, 1.0)
    };
    let exp = if gamma <= f32::EPSILON {
        1.0
    } else {
        1.0 / gamma
    };
    out_black + (out_white - out_black) * norm.powf(exp)
}

/// Bake the eight levels transfers into the shared LUT + gate flags. The channel
/// semantics (composite fold, HSL rows, gating) live in [`bake_lut`].
fn build_lut(params: &[ParamValue]) -> Baked {
    let arrs: Vec<[f32; 5]> = (0..PARAMS.len())
        .map(|i| levels_params(params, i))
        .collect();
    bake_lut(|ch, t| levels_transfer(&arrs[ch as usize], t))
}

fn create_pipeline(device: &wgpu::Device) -> Arc<dyn FilterEffect> {
    Arc::new(lut_param_filter(device, &lut_shader_source(), build_lut))
}

/// The input range pinches inward against a brightening gamma, then opens back
/// out against a darkening one, and returns — the two halves of what the
/// control does, in one pass.
///
/// `gamma` is a raw exponent rather than a perceptual scale, so it sweeps as a
/// ratio around 1.0 rather than an even numeric spread: the two extremes are
/// reciprocals and read as equal and opposite. Only the composite `rgb` channel
/// moves; the other seven stay at their identity defaults.
fn preview_params(t: f32) -> Vec<ParamValue> {
    let s = swing_signed(t);
    let pinch = 0.15 * s.max(0.0);
    let mut params: Vec<ParamValue> = PARAMS.iter().map(ParamDef::default_value).collect();
    params[0] = ParamValue::Levels([
        // rgb
        pinch,
        1.0 - pinch,
        2.2f32.powf(-s),
        0.0,
        1.0,
    ]);
    params
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "levels",
        display_name: "Levels",
        icon: "fa6-solid:sliders",
        description: "Tone mapping with black point, white point, gamma, and output range.",
        hotkey_action: "filterLevels",
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

    fn levels(a: [f32; 5]) -> ParamValue {
        ParamValue::Levels(a)
    }

    /// Row 0, component `c` (0=r,1=g,2=b,3=a) at input index `i`.
    fn row0(lut: &[u8; LUT_BYTES], i: usize, c: usize) -> u8 {
        lut[i * 4 + c]
    }
    /// Row 1, component `c` (0=hue,1=sat,2=lightness) at input index `i`.
    fn row1(lut: &[u8; LUT_BYTES], i: usize, c: usize) -> u8 {
        lut[LUT_LEN * 4 + i * 4 + c]
    }

    const RGB: usize = Channel::Rgb as usize;
    const RED: usize = Channel::Red as usize;
    const SATURATION: usize = Channel::Saturation as usize;
    const LIGHTNESS: usize = Channel::Lightness as usize;

    /// Default (identity) levels ⇒ identity LUT on every channel (both rows),
    /// and neither gated stage armed — an all-default Levels layer is a no-op.
    #[test]
    fn identity_levels_yield_identity_lut() {
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
            "identity levels must not arm the HSV pass"
        );
        assert!(
            !baked.lightness_active,
            "identity levels must not arm the Lab pass"
        );
    }

    /// The gamma transfer maps the midpoint to the hand-computed value:
    /// gamma 2 over the full range is `x^(1/2)`, so `0.25 → 0.5`.
    #[test]
    fn gamma_maps_midpoint() {
        let p = [0.0, 1.0, 2.0, 0.0, 1.0];
        assert!(
            (levels_transfer(&p, 0.25) - 0.5).abs() < 1e-5,
            "sqrt(0.25) = 0.5, got {}",
            levels_transfer(&p, 0.25)
        );
        // A linear window: inBlack..inWhite = 0.2..0.8, gamma 1 → its centre 0.5
        // sits at the output centre 0.5.
        let q = [0.2, 0.8, 1.0, 0.0, 1.0];
        assert!((levels_transfer(&q, 0.5) - 0.5).abs() < 1e-5);
    }

    /// Output range clamps to `[outBlack, outWhite]`: inputs at/below `inBlack`
    /// map to `outBlack`, at/above `inWhite` to `outWhite`, and the interior
    /// remaps linearly into that output window.
    #[test]
    fn output_range_clamps() {
        let p = [0.2, 0.8, 1.0, 0.1, 0.9];
        assert!((levels_transfer(&p, 0.0) - 0.1).abs() < 1e-6);
        assert!((levels_transfer(&p, 0.2) - 0.1).abs() < 1e-6);
        assert!((levels_transfer(&p, 1.0) - 0.9).abs() < 1e-6);
        // Centre of the input window → centre of the output window.
        assert!((levels_transfer(&p, 0.5) - 0.5).abs() < 1e-6);
    }

    /// Inverted output (`outBlack > outWhite`) is permitted and produces a
    /// descending transfer — the Krita formula falls out of it unchanged.
    #[test]
    fn inverted_output_descends() {
        let p = [0.0, 1.0, 1.0, 1.0, 0.0];
        assert!((levels_transfer(&p, 0.0) - 1.0).abs() < 1e-6);
        assert!((levels_transfer(&p, 1.0) - 0.0).abs() < 1e-6);
        assert!((levels_transfer(&p, 0.25) - 0.75).abs() < 1e-6);
    }

    /// Fold order matches Krita: the color channels are `rgb(channel(i))` — the
    /// per-channel transfer first, then the composite "RGB" transfer on top.
    #[test]
    fn composite_composes_over_channel() {
        // rgb composite doubles+clamps (inWhite 0.5); red halves (outWhite 0.5).
        // rgb(red(t)) = min(2·(0.5·t), 1) = min(t, 1) = t → identity.
        let mut params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        params[RGB] = levels([0.0, 0.5, 1.0, 0.0, 1.0]); // doubles, clamps
        params[RED] = levels([0.0, 1.0, 1.0, 0.0, 0.5]); // halves
        let baked = build_lut(&params);
        assert_eq!(row0(&baked.lut, 255, 0), 255, "rgb(red(255)) must be 255");
        assert!(
            (row0(&baked.lut, 128, 0) as i32 - 128).abs() <= 2,
            "rgb(red(0.5)) ≈ 0.5, got {}",
            row0(&baked.lut, 128, 0)
        );
        // Composite never touches alpha.
        for i in 0..LUT_LEN {
            assert_eq!(
                row0(&baked.lut, i, 3) as usize,
                i,
                "alpha stays identity {i}"
            );
        }
    }

    /// A non-identity Hue or Saturation transfer arms the HSV pass; a
    /// non-identity Lightness transfer arms the Lab pass — independently.
    #[test]
    fn hsl_levels_arm_their_stages() {
        let mut p = PARAMS.iter().map(|d| d.default_value()).collect::<Vec<_>>();
        p[SATURATION] = levels([0.0, 1.0, 1.0, 0.0, 0.5]); // halves saturation
        let baked = build_lut(&p);
        assert!(
            baked.hsv_active,
            "a saturation transfer must arm the HSV pass"
        );
        assert!(!baked.lightness_active);

        let mut p = PARAMS.iter().map(|d| d.default_value()).collect::<Vec<_>>();
        p[LIGHTNESS] = levels([0.0, 1.0, 1.0, 0.0, 0.5]);
        let baked = build_lut(&p);
        assert!(
            baked.lightness_active,
            "a lightness transfer must arm the Lab pass"
        );
        assert!(!baked.hsv_active);
    }
}
