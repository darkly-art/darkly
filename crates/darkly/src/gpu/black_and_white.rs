//! Black and White — the shared core behind the `black_and_white` veil
//! (`gpu/veils/black_and_white.rs`) and filter
//! (`gpu/filters/black_and_white.rs`). One identity, one param schema, one
//! uniform layout, one WGSL transform
//! ([`black_and_white.wgsl`](../../shaders/lib/black_and_white.wgsl)); the two
//! surfaces are thin wrappers over their respective pipeline infrastructures
//! (`EffectPipeline` for the veil, `ParamFilter` for the filter).
//!
//! The six fixed gray formulas match Krita's desaturate adjustment
//! (`plugins/color/colorspaceextensions/kis_desaturate_adjustment.cpp`), which
//! follows Tanner Helland's grayscale algorithm survey
//! (<https://www.tannerhelland.com/3643/grayscale-image-algorithm-vb6/>).
//! Mode 6 is a custom weighted mix, and an optional hue tint colors the gray.

use crate::gpu::params::{ConstParamValue, ParamDef, ParamValue};
use crate::gpu::preview::{ANIMATED_FRAMES, PREVIEW_FPS};
use crate::gpu::preview_recipe::{Key, PreviewRecipe, Track, TrackTarget};

pub const TYPE_ID: &str = "black_and_white";
pub const DISPLAY_NAME: &str = "Black and White";
pub const DESCRIPTION: &str = "Desaturate to black and white — six grayscale \
formulas or custom channel weights, with an optional color tint.";

/// One schema for both surfaces. The weights only take effect in the
/// `Custom Weights` mode (their defaults are the BT.601 luma coefficients);
/// the tint applies in every mode. A `static` rather than a `const` so both
/// registrations hold the same address — pinned by the identity test below.
pub static PARAMS: &[ParamDef] = &[
    ParamDef::enumeration(
        "mode",
        &[
            "Lightness",
            "Luminosity (BT.709)",
            "Luminosity (BT.601)",
            "Average",
            "Min",
            "Max",
            "Custom Weights",
        ],
        0,
    )
    .with_label("Mode")
    .with_description("How colour is weighed when collapsing it to grey."),
    ParamDef::float("red_weight", 0.0, 1.0, 0.299)
        .with_label("Red Weight")
        .with_description("How much the red channel contributes, in Custom Weights mode."),
    ParamDef::float("green_weight", 0.0, 1.0, 0.587)
        .with_label("Green Weight")
        .with_description("How much the green channel contributes, in Custom Weights mode."),
    ParamDef::float("blue_weight", 0.0, 1.0, 0.114)
        .with_label("Blue Weight")
        .with_description("How much the blue channel contributes, in Custom Weights mode."),
    ParamDef::float("tint_hue", 0.0, 360.0, 0.0)
        .with_label("Tint Hue")
        .with_description("Which colour the finished grey is toned toward."),
    ParamDef::float("tint_strength", 0.0, 1.0, 0.0)
        .with_label("Tint Strength")
        .with_description("How strongly the tint colour shows through the grey."),
];

/// One recipe for both surfaces, beside the schema they share. The grey is
/// toned through the full colour wheel while the tint strengthens and fades, so
/// a single pass shows both the desaturation and what the tint controls do to
/// it. A `static` for the same reason `PARAMS` is one — both registrations hold
/// the same address, which is what makes the sharing structural rather than two
/// copies that happen to agree today.
pub static PREVIEW: PreviewRecipe = PreviewRecipe {
    frames: ANIMATED_FRAMES,
    fps: PREVIEW_FPS,
    tracks: &[
        Track {
            target: TrackTarget::Param("tint_strength"),
            keys: &[
                Key {
                    t: 0.0,
                    value: ConstParamValue::Float(0.0),
                },
                Key {
                    t: 0.5,
                    value: ConstParamValue::Float(1.0),
                },
                Key {
                    t: 1.0,
                    value: ConstParamValue::Float(0.0),
                },
            ],
        },
        Track {
            target: TrackTarget::Param("tint_hue"),
            keys: &[
                Key {
                    t: 0.0,
                    value: ConstParamValue::Float(0.0),
                },
                Key {
                    t: 0.5,
                    value: ConstParamValue::Float(360.0),
                },
                Key {
                    t: 1.0,
                    value: ConstParamValue::Float(0.0),
                },
            ],
        },
    ],
};

/// The shared WGSL transform (`BwParams` / `bw_gray` / `bw_transform`),
/// prepended to each surface's wrapper shader at pipeline build time.
pub const SHADER_LIB: &str = include_str!("../../shaders/lib/black_and_white.wgsl");

/// Positional float lookup with schema-default fallback — params arrive
/// positionally, like every pack in `gpu/filters/`.
fn float_param(params: &[ParamValue], idx: usize) -> f32 {
    if let Some(ParamValue::Float(v)) = params.get(idx) {
        return *v;
    }
    match PARAMS[idx].default_value() {
        ParamValue::Float(v) => v,
        _ => 0.0,
    }
}

/// Pack the shared schema into the shader's 32-byte `BwParams` uniform:
/// `[mode: u32, red_w, green_w, blue_w, tint_r, tint_g, tint_b,
/// tint_strength]` — floats stored as bit patterns beside the u32 (the same
/// packing `gpu/filters/hsv.rs` uses). Missing or mistyped entries fall back
/// to the schema defaults.
///
/// Two conversions happen here rather than per pixel in the shader: the
/// custom weights are normalized to sum 1 (⅓ each when all ~zero), and the
/// tint hue becomes an RGB color.
pub fn pack_uniform(params: &[ParamValue]) -> [u32; 8] {
    let mode = match params.first() {
        Some(ParamValue::Int(m)) => (*m).max(0) as u32,
        _ => 0,
    };
    let w = [
        float_param(params, 1),
        float_param(params, 2),
        float_param(params, 3),
    ];
    let sum: f32 = w.iter().sum();
    let w = if sum < 0.001 {
        [1.0 / 3.0; 3]
    } else {
        w.map(|v| v / sum)
    };
    let tint = hue_to_rgb(float_param(params, 4));
    [
        mode,
        w[0].to_bits(),
        w[1].to_bits(),
        w[2].to_bits(),
        tint[0].to_bits(),
        tint[1].to_bits(),
        tint[2].to_bits(),
        float_param(params, 5).to_bits(),
    ]
}

/// Hue (degrees) → fully saturated, full-value RGB — the s = v = 1 slice of
/// `hsv_to_rgb` in `shaders/lib/colorspace.wgsl`, ported to Rust so the tint
/// costs nothing per pixel.
fn hue_to_rgb(h_deg: f32) -> [f32; 3] {
    let h = h_deg.rem_euclid(360.0) / 60.0;
    let f = h - h.floor();
    // With s = v = 1: p = 0, q = 1 - f, t = f.
    match h as u32 {
        0 => [1.0, f, 0.0],
        1 => [1.0 - f, 1.0, 0.0],
        2 => [0.0, 1.0, f],
        3 => [0.0, 1.0 - f, 1.0],
        4 => [f, 0.0, 1.0],
        _ => [1.0, 0.0, 1.0 - f],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_at(u: &[u32; 8], i: usize) -> f32 {
        f32::from_bits(u[i])
    }

    fn defaults() -> Vec<ParamValue> {
        PARAMS.iter().map(|d| d.default_value()).collect()
    }

    /// Default params → mode 0 (Lightness, Krita's default), tint off.
    #[test]
    fn defaults_pack_to_lightness() {
        let u = pack_uniform(&defaults());
        assert_eq!(u[0], 0, "default mode is Lightness");
        assert_eq!(f32_at(&u, 7), 0.0, "tint off by default");
    }

    /// An explicit mode passes straight through as u32.
    #[test]
    fn explicit_mode_passes_through() {
        let u = pack_uniform(&[ParamValue::Int(5)]);
        assert_eq!(u[0], 5, "mode Max");
    }

    /// An empty (or partial) param vec packs the schema defaults — callers
    /// like `apply_filter_typed` pass positional prefixes.
    #[test]
    fn missing_params_fall_back_to_defaults() {
        let full = pack_uniform(&defaults());
        let empty = pack_uniform(&[]);
        assert_eq!(full, empty, "empty vec must equal explicit defaults");
    }

    /// Custom weights are normalized to sum 1 before upload.
    #[test]
    fn custom_weights_normalize() {
        let u = pack_uniform(&[
            ParamValue::Int(6),
            ParamValue::Float(1.0),
            ParamValue::Float(0.5),
            ParamValue::Float(0.5),
        ]);
        assert_eq!(f32_at(&u, 1), 0.5);
        assert_eq!(f32_at(&u, 2), 0.25);
        assert_eq!(f32_at(&u, 3), 0.25);
    }

    /// All-zero weights degrade to an even ⅓ mix, not a division by zero.
    #[test]
    fn zero_weights_degrade_to_average() {
        let u = pack_uniform(&[
            ParamValue::Int(6),
            ParamValue::Float(0.0),
            ParamValue::Float(0.0),
            ParamValue::Float(0.0),
        ]);
        for i in 1..=3 {
            assert!((f32_at(&u, i) - 1.0 / 3.0).abs() < 1e-6);
        }
    }

    /// Tint hue converts to RGB at pack time (s = v = 1 HSV slice).
    #[test]
    fn tint_hue_converts_to_rgb() {
        for (hue, rgb) in [
            (0.0, [1.0, 0.0, 0.0]),
            (120.0, [0.0, 1.0, 0.0]),
            (240.0, [0.0, 0.0, 1.0]),
            (360.0, [1.0, 0.0, 0.0]),
        ] {
            let mut params = defaults();
            params[4] = ParamValue::Float(hue);
            let u = pack_uniform(&params);
            for (i, want) in rgb.iter().enumerate() {
                assert!(
                    (f32_at(&u, 4 + i) - want).abs() < 1e-6,
                    "hue {hue}° channel {i}: got {}, want {want}",
                    f32_at(&u, 4 + i)
                );
            }
        }
    }

    /// The veil and filter registries expose the *same* black-and-white:
    /// identical schema const (by pointer — compile-level DRY), identical
    /// display name, and a description containing "desaturate" so the
    /// command-palette substring search finds it under its old name.
    #[test]
    fn veil_and_filter_share_one_identity() {
        let veil = crate::gpu::veils::registrations()
            .into_iter()
            .find(|r| r.type_id == TYPE_ID)
            .expect("black_and_white veil registered");
        let filter = crate::gpu::filters::registrations()
            .into_iter()
            .find(|r| r.type_id == TYPE_ID)
            .expect("black_and_white filter registered");
        assert!(
            std::ptr::eq(veil.params.as_ptr(), filter.params.as_ptr())
                && veil.params.len() == filter.params.len(),
            "both surfaces must reference the shared PARAMS const"
        );
        assert_eq!(veil.display_name, filter.display_name);
        assert_eq!(veil.display_name, DISPLAY_NAME);
        for description in [veil.description, filter.description] {
            assert!(
                description.to_lowercase().contains("desaturate"),
                "palette search for 'desaturate' must find Black and White"
            );
        }
    }
}
