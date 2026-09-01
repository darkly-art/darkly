//! Brightness/Contrast filter — the classic two-slider adjustment, using
//! GIMP's mapping (Michael Natterer,
//! `gimp/app/operations/gimpoperationbrightnesscontrast.c`,
//! <https://gitlab.gnome.org/GNOME/gimp/-/blob/master/app/operations/gimpoperationbrightnesscontrast.c>):
//!
//! - brightness `b ∈ −1..1`, halved: `v·(1+b)` below zero, `v + (1−v)·b` above
//! - contrast `c ∈ −1..1` as a slant through mid-gray: `(v − 0.5)·tan((c+1)·π/4) + 0.5`
//!
//! Applied per RGB channel, alpha untouched. Like HSV this is the no-aux
//! [`ParamFilter`] specialization (`[src, uniform]`) — the transform lives in
//! [`brightness_contrast.wgsl`](../../../shaders/effects/brightness_contrast.wgsl);
//! this module declares the param schema and packs the two sliders
//! (−100..100, Darkly's convention) into shader-ready `brightness`/`slant`.

use std::sync::Arc;

use crate::gpu::effect::{
    create_effect_pipeline, Binding, EffectPipeline, EffectRegistration, COLOR_TARGETS,
};
use crate::gpu::param_effect::{ParamEffectKind, Resources};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::preview::{swing_signed, PreviewAnim};

pub const PARAMS: &[ParamDef] = &[
    ParamDef::float("brightness", -100.0, 100.0, 0.0)
        .with_label("Brightness")
        .with_description("Lifts or lowers every tone by the same amount."),
    ParamDef::float("contrast", -100.0, 100.0, 0.0)
        .with_label("Contrast")
        .with_description("Spreads tones away from mid grey, or gathers them toward it."),
];

fn float_param(params: &[ParamValue], idx: usize) -> f32 {
    match params.get(idx) {
        Some(ParamValue::Float(v)) => *v,
        _ => 0.0,
    }
}

/// Pack the two sliders into the shader's `Params` uniform layout (32 bytes):
/// `[brightness, slant, pad×6]`. The shader-ready values are precomputed here —
/// `brightness = slider/100/2` (GIMP halves it) and `slant = tan((c+1)·π/4)`.
///
/// `contrast == 0.0` maps to `slant = 1.0` *exactly* rather than through `tan`,
/// whose π/4 result is only approximately 1 — the shader's identity fast path
/// (and bit-exact no-op guarantee) gates on `slant == 1.0`. The `tan` itself
/// runs in f64 (as GIMP's does): the f32 nearest to π/2 lands *above* the true
/// value, so an f32 `tan` at contrast +100 would flip to a huge negative slant.
fn pack_uniform(params: &[ParamValue]) -> [u32; 8] {
    let brightness = float_param(params, 0) / 100.0 / 2.0; // → −0.5..0.5
    let contrast = f64::from(float_param(params, 1)) / 100.0; // → −1..1
    let slant = if contrast == 0.0 {
        1.0
    } else {
        ((contrast + 1.0) * std::f64::consts::FRAC_PI_4).tan() as f32
    };
    [brightness.to_bits(), slant.to_bits(), 0, 0, 0, 0, 0, 0]
}

const BINDINGS: &[Binding] = &[Binding::Texture, Binding::Uniform];

fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "brightness-contrast",
        BINDINGS,
        include_str!("../../../shaders/effects/brightness_contrast.wgsl"),
        "fs_bc",
    )
}

fn kind() -> Arc<ParamEffectKind> {
    ParamEffectKind::new(
        "brightness_contrast",
        "brightness-contrast",
        PARAMS,
        BINDINGS,
        Resources::Packed(|params| bytemuck::cast_slice(&pack_uniform(params)).to_vec()),
    )
}

/// Both sliders swing up, down and back, concurrently — so the preview shows
/// the two controls interacting rather than one at a time. Contrast leads with
/// a wider positive swing because it reads more slowly than brightness at the
/// same magnitude, and a narrower negative one because flattening reads faster
/// than steepening.
fn preview_params(t: f32) -> Vec<ParamValue> {
    let s = swing_signed(t);
    let mut params: Vec<ParamValue> = PARAMS.iter().map(ParamDef::default_value).collect();
    params[0] = ParamValue::Float(40.0 * s); // brightness
    params[1] = ParamValue::Float(60.0 * s.max(0.0) + 40.0 * s.min(0.0)); // contrast
    params
}

pub fn register() -> EffectRegistration {
    EffectRegistration {
        type_id: "brightness_contrast",
        display_name: "Brightness/Contrast",
        category: "Filters",
        icon: "fa6-solid:sun",
        description: "The classic two-slider brightness and contrast adjustment.",
        hotkey_action: "effectBrightness_contrast",
        params: PARAMS,
        // A signed sweep rests in the middle, so the default still would be the
        // frame that looks like no effect at all. The quarter point is its
        // positive extreme.
        preview: Some(PreviewAnim::LOOPING.with_still_at(0.25)),
        preview_at: Some(preview_params),
        targets: COLOR_TARGETS,
        create_pipeline,
        from_params: |params, shared| kind().instance(params, shared),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default params → exact identity: brightness 0 and slant *bit-exact* 1.0
    /// (the `contrast == 0` special case bypasses `tan`, whose π/4 result is
    /// only approximately 1), so the shader's no-op fast path engages.
    #[test]
    fn defaults_pack_to_identity() {
        let params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        let u = pack_uniform(&params);
        assert_eq!(f32::from_bits(u[0]), 0.0, "brightness");
        assert_eq!(u[1], 1.0f32.to_bits(), "slant bit-exact 1.0");
    }

    /// Slider extremes normalize per GIMP: brightness ±100 → ±0.5 (halved),
    /// contrast −100 → slant 0 (flat mid-gray), contrast +100 → a steep slant.
    #[test]
    fn packs_normalize_extremes() {
        let u = pack_uniform(&[ParamValue::Float(100.0), ParamValue::Float(-100.0)]);
        assert_eq!(f32::from_bits(u[0]), 0.5, "brightness 100 → 0.5");
        assert_eq!(f32::from_bits(u[1]), 0.0, "contrast −100 → slant 0");

        let u = pack_uniform(&[ParamValue::Float(-100.0), ParamValue::Float(100.0)]);
        assert_eq!(f32::from_bits(u[0]), -0.5, "brightness −100 → −0.5");
        assert!(
            f32::from_bits(u[1]) > 1000.0,
            "contrast 100 → slant tan(π/2), effectively vertical"
        );
    }
}
