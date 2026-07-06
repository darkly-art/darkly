//! Hue/Saturation filter — hue rotation plus saturation/value scaling, modeled
//! on Krita's `hsvadjustment`
//! (`plugins/color/colorspaceextensions/kis_hsv_adjustment.cpp`).
//!
//! Four modes over one shader: three colour models — **HSV, HSL, HSY**
//! (luma-weighted HCY; Krita's "HSY" *is* HCY) — plus **Colorize** (absolute
//! hue/saturation with luminance preserved, like Photoshop's Hue/Saturation
//! colorize), which overrides the model selector. The transform lives entirely
//! in [`hsv.wgsl`](../../../shaders/filters/hsv.wgsl); this module declares the
//! param schema and packs the params into the shader's uniform.
//!
//! Unlike Curves/Levels this filter carries **no auxiliary texture** — it builds
//! the no-aux [`ParamFilter`] specialization (`[src, uniform]`), packing the five
//! params into a single 32-byte uniform. Ranges follow Krita
//! (`kis_hsv_adjustment_filter.cpp`): hue −180..180°, saturation/value −100..100,
//! normalized here to −1..1 for the shader.

use std::sync::Arc;

use crate::gpu::effect::EffectCache;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::param_filter::ParamFilter;
use crate::gpu::params::{ParamDef, ParamValue};

/// Parameter schema. `model` is an enum dropdown; the three scalars are plain
/// rows; `colorize` is a checkbox that (in the shader) overrides the model.
pub const PARAMS: &[ParamDef] = &[
    ParamDef::Enum {
        name: "model",
        options: &["HSV", "HSL", "HSY"],
        default: 0,
    },
    ParamDef::Float {
        name: "hue",
        min: -180.0,
        max: 180.0,
        default: 0.0,
    },
    ParamDef::Float {
        name: "saturation",
        min: -100.0,
        max: 100.0,
        default: 0.0,
    },
    ParamDef::Float {
        name: "value",
        min: -100.0,
        max: 100.0,
        default: 0.0,
    },
    ParamDef::Bool {
        name: "colorize",
        default: false,
    },
];

/// The HSV fragment shader: the shared colour-space lib prepended to `hsv.wgsl`
/// (built at load time — the render shaders have no `#include` preprocessor).
fn hsv_shader_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../../../shaders/lib/colorspace.wgsl"),
        include_str!("../../../shaders/filters/hsv.wgsl"),
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
/// `prepare` half for the no-aux HSV filter.
fn hsv_prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    params: &[ParamValue],
    cache: &mut EffectCache,
) {
    if cache.uniform_bufs.is_empty() {
        cache
            .uniform_bufs
            .push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("filter-hsv-uniform"),
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
        "filter-hsv",
        &hsv_shader_source(),
        "fs_hsv",
        "fs_hsv_masked",
        false, // no aux texture — packed uniform only
        hsv_prepare,
    ))
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "hsv",
        display_name: "Hue/Saturation",
        params: PARAMS,
        create_pipeline,
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
