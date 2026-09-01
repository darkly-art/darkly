//! Hue/Saturation — hue rotation plus saturation/value scaling, modeled
//! on Krita's `hsvadjustment`
//! (`plugins/color/colorspaceextensions/kis_hsv_adjustment.cpp`).
//!
//! Four modes over one shader: three colour models — **HSV, HSL, HSY**
//! (luma-weighted HCY; Krita's "HSY" *is* HCY) — plus **Colorize** (absolute
//! hue/saturation with luminance preserved, like Photoshop's Hue/Saturation
//! colorize), which overrides the model selector. The transform lives entirely
//! in [`hsv.wgsl`](../../../shaders/effects/hsv.wgsl); this module declares the
//! param schema and packs the params into the shader's uniform.
//!
//! Unlike Curves/Levels this carries **no auxiliary texture**: `[src, uniform]`,
//! the five params packed into a single 32-byte uniform. Ranges follow Krita
//! (`kis_hsv_adjustment_filter.cpp`): hue −180..180°, saturation/value −100..100,
//! normalized here to −1..1 for the shader.

use std::sync::Arc;

use crate::gpu::effect::{
    create_effect_pipeline, Binding, EffectPipeline, EffectRegistration, COLOR_TARGETS,
};
use crate::gpu::param_effect::{ParamEffectKind, Resources};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::preview::{swing, swing_signed, PreviewAnim};

/// Parameter schema. `model` is an enum dropdown; the three scalars are plain
/// rows; `colorize` is a checkbox that (in the shader) overrides the model.
pub const PARAMS: &[ParamDef] = &[
    ParamDef::enumeration("model", &["HSV", "HSL", "HSY"], 0)
        .with_label("Color Model")
        .with_description("Which cylindrical model the adjustment works in."),
    ParamDef::float("hue", -180.0, 180.0, 0.0)
        .with_label("Hue")
        .with_description("Rotation applied to every pixel's hue."),
    ParamDef::float("saturation", -100.0, 100.0, 0.0)
        .with_label("Saturation")
        .with_description("Pushes colors toward grey or toward full intensity."),
    ParamDef::float("value", -100.0, 100.0, 0.0)
        .with_label("Value")
        .with_description("Lightens or darkens without changing hue."),
    ParamDef::boolean("colorize", false)
        .with_label("Colorize")
        .with_description(
            "Replaces every hue with the chosen one, tinting the layer a single color.",
        ),
];

/// The HSV fragment shader: the shared colour-space lib prepended to `hsv.wgsl`
/// (built at load time — the render shaders have no `#include` preprocessor).
fn hsv_shader_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../../../shaders/lib/colorspace.wgsl"),
        include_str!("../../../shaders/effects/hsv.wgsl"),
    )
}

fn float_param(params: &[ParamValue], idx: usize) -> f32 {
    match params.get(idx) {
        Some(ParamValue::Float(v)) => *v,
        _ => 0.0,
    }
}

/// Pack the five params into the shader's `Params` uniform layout (32 bytes):
/// `[hue°, saturation/100, value/100, model:u32, colorize:u32, pad×3]`. The
/// three floats are stored as their bit patterns alongside the two u32s.
fn pack_uniform(params: &[ParamValue]) -> [u32; 8] {
    let hue = float_param(params, 1); // degrees, already −180..180
    let saturation = float_param(params, 2) / 100.0; // → −1..1
    let value = float_param(params, 3) / 100.0; // → −1..1
    let model = match params.first() {
        Some(ParamValue::Int(m)) => (*m).max(0) as u32,
        _ => 0,
    };
    let colorize = matches!(params.get(4), Some(ParamValue::Bool(true))) as u32;
    [
        hue.to_bits(),
        saturation.to_bits(),
        value.to_bits(),
        model,
        colorize,
        0,
        0,
        0,
    ]
}

/// Allocate (once) and refresh the params uniform — the [`ParamFilter`]
const BINDINGS: &[Binding] = &[Binding::Texture, Binding::Uniform];

fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "hsv",
        BINDINGS,
        &hsv_shader_source(),
        "fs_hsv",
    )
}

fn kind() -> Arc<ParamEffectKind> {
    ParamEffectKind::new(
        "hsv",
        "hsv",
        PARAMS,
        BINDINGS,
        Resources::Packed(|params| bytemuck::cast_slice(&pack_uniform(params)).to_vec()),
    )
}

/// The image lightens, darkens and returns while the hue rotates out and back,
/// the two sweeps running concurrently. `model` and `colorize` stay at their
/// defaults, so what moves is exactly the pair of knobs the filter is named
/// for. A full 360° spin is expressible as `hue: -180 → 180` — the parameter's
/// own endpoints are the same colour — but it would not end where it began, so
/// the ping-pong closes instead.
fn preview_params(t: f32) -> Vec<ParamValue> {
    let mut params: Vec<ParamValue> = PARAMS.iter().map(ParamDef::default_value).collect();
    params[1] = ParamValue::Float(180.0 * swing(t)); // hue
    params[3] = ParamValue::Float(60.0 * swing_signed(t)); // value
    params
}

pub fn register() -> EffectRegistration {
    EffectRegistration {
        type_id: "hsv",
        display_name: "Hue/Saturation",
        category: "Filters",
        icon: "fa6-solid:palette",
        description: "Rotate hue and scale saturation and value, with optional colorize.",
        hotkey_action: "effectHsv",
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

    /// Default params → identity adjustment: model HSV, all scalars 0, colorize
    /// off. The uniform's colorize flag is 0 and the deltas are 0, which the
    /// shader's fast path reads as a no-op.
    #[test]
    fn defaults_pack_to_identity() {
        let params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        let u = pack_uniform(&params);
        assert_eq!(f32::from_bits(u[0]), 0.0, "hue");
        assert_eq!(f32::from_bits(u[1]), 0.0, "saturation");
        assert_eq!(f32::from_bits(u[2]), 0.0, "value");
        assert_eq!(u[3], 0, "model HSV");
        assert_eq!(u[4], 0, "colorize off");
    }

    /// −100..100 slider ranges normalize to −1..1; the model enum and colorize
    /// bool pass straight through as u32.
    #[test]
    fn packs_normalize_and_flags() {
        let params = vec![
            ParamValue::Int(2),      // model HSY
            ParamValue::Float(90.0), // hue
            ParamValue::Float(-100.0),
            ParamValue::Float(50.0),
            ParamValue::Bool(true), // colorize
        ];
        let u = pack_uniform(&params);
        assert_eq!(f32::from_bits(u[0]), 90.0);
        assert_eq!(f32::from_bits(u[1]), -1.0, "saturation −100 → −1");
        assert_eq!(f32::from_bits(u[2]), 0.5, "value 50 → 0.5");
        assert_eq!(u[3], 2, "model HSY");
        assert_eq!(u[4], 1, "colorize on");
    }
}
