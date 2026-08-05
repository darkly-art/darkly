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
//! [`brightness_contrast.wgsl`](../../../shaders/filters/brightness_contrast.wgsl);
//! this module declares the param schema and packs the two sliders
//! (−100..100, Darkly's convention) into shader-ready `brightness`/`slant`.

use std::sync::Arc;

use crate::gpu::effect::EffectCache;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::param_filter::{ParamFilter, SrcSampling};
use crate::gpu::params::{ConstParamValue, ParamDef, ParamValue};
use crate::gpu::preview::{ANIMATED_FRAMES, PREVIEW_FPS};
use crate::gpu::preview_recipe::{Key, PreviewRecipe, Track, TrackTarget};

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

/// Allocate (once) and refresh the params uniform — the [`ParamFilter`]
/// `prepare` half for the no-aux brightness/contrast filter.
fn bc_prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    params: &[ParamValue],
    cache: &mut EffectCache,
) {
    if cache.uniform_bufs.is_empty() {
        cache
            .uniform_bufs
            .push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("filter-brightness-contrast-uniform"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
    }
    queue.write_buffer(
        &cache.uniform_bufs[0],
        0,
        bytemuck::cast_slice(&pack_uniform(params)),
    );
}

fn create_pipeline(device: &wgpu::Device) -> Arc<dyn FilterEffect> {
    Arc::new(ParamFilter::new(
        device,
        "filter-brightness-contrast",
        include_str!("../../../shaders/filters/brightness_contrast.wgsl"),
        "fs_bc",
        "fs_bc_masked",
        false, // no aux texture — packed uniform only
        SrcSampling::Load,
        bc_prepare,
    ))
}

/// Both sliders swing up, down and back, concurrently — so the preview shows
/// the two controls interacting rather than one at a time. Contrast leads with a
/// wider swing because it reads more slowly than brightness at the same
/// magnitude.
static PREVIEW: PreviewRecipe = PreviewRecipe {
    frames: ANIMATED_FRAMES,
    fps: PREVIEW_FPS,
    tracks: &[
        Track {
            target: TrackTarget::Param("brightness"),
            keys: &[
                Key {
                    t: 0.00,
                    value: ConstParamValue::Float(0.0),
                },
                Key {
                    t: 0.25,
                    value: ConstParamValue::Float(40.0),
                },
                Key {
                    t: 0.75,
                    value: ConstParamValue::Float(-40.0),
                },
                Key {
                    t: 1.00,
                    value: ConstParamValue::Float(0.0),
                },
            ],
        },
        Track {
            target: TrackTarget::Param("contrast"),
            keys: &[
                Key {
                    t: 0.00,
                    value: ConstParamValue::Float(0.0),
                },
                Key {
                    t: 0.25,
                    value: ConstParamValue::Float(60.0),
                },
                Key {
                    t: 0.75,
                    value: ConstParamValue::Float(-40.0),
                },
                Key {
                    t: 1.00,
                    value: ConstParamValue::Float(0.0),
                },
            ],
        },
    ],
};

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "brightness_contrast",
        display_name: "Brightness/Contrast",
        icon: "fa6-solid:sun",
        description: "The classic two-slider brightness and contrast adjustment.",
        params: PARAMS,
        preview: Some(&PREVIEW),
        create_pipeline,
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
