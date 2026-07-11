//! Desaturate filter — one-step RGB → gray conversion, modeled on Krita's
//! desaturate adjustment
//! (`plugins/color/colorspaceextensions/kis_desaturate_adjustment.cpp`), which
//! follows Tanner Helland's grayscale algorithm survey
//! (<https://www.tannerhelland.com/3643/grayscale-image-algorithm-vb6/>).
//!
//! Six modes over one shader, selected by a single enum param (Krita's order,
//! default Lightness): lightness `(max+min)/2`, luminosity BT.709 / BT.601,
//! average `(r+g+b)/3`, min, max. Distinct from Hue/Saturation, whose
//! saturation −100 reaches only one gray mapping per color model.
//!
//! Like HSV this is the no-aux [`ParamFilter`] specialization
//! (`[src, uniform]`) — the transform lives in
//! [`desaturate.wgsl`](../../../shaders/filters/desaturate.wgsl); this module
//! declares the param schema and packs the mode into the shader's uniform.

use std::sync::Arc;

use crate::gpu::effect::EffectCache;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::param_filter::ParamFilter;
use crate::gpu::params::{ParamDef, ParamValue};

pub const PARAMS: &[ParamDef] = &[ParamDef::Enum {
    name: "mode",
    options: &[
        "Lightness",
        "Luminosity (BT.709)",
        "Luminosity (BT.601)",
        "Average",
        "Min",
        "Max",
    ],
    default: 0,
}];

/// Pack the mode enum into the shader's `Params` uniform layout (32 bytes):
/// `[mode: u32, pad×7]`.
fn pack_uniform(params: &[ParamValue]) -> [u32; 8] {
    let mode = match params.first() {
        Some(ParamValue::Int(m)) => (*m).max(0) as u32,
        _ => 0,
    };
    [mode, 0, 0, 0, 0, 0, 0, 0]
}

/// Allocate (once) and refresh the params uniform — the [`ParamFilter`]
/// `prepare` half for the no-aux desaturate filter.
fn desaturate_prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    params: &[ParamValue],
    cache: &mut EffectCache,
) {
    if cache.uniform_bufs.is_empty() {
        cache
            .uniform_bufs
            .push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("filter-desaturate-uniform"),
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
        "filter-desaturate",
        include_str!("../../../shaders/filters/desaturate.wgsl"),
        "fs_desaturate",
        "fs_desaturate_masked",
        false, // no aux texture — packed uniform only
        desaturate_prepare,
    ))
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "desaturate",
        display_name: "Desaturate",
        params: PARAMS,
        create_pipeline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default params → mode 0 (Lightness, Krita's default).
    #[test]
    fn defaults_pack_to_lightness() {
        let params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        let u = pack_uniform(&params);
        assert_eq!(u[0], 0, "default mode is Lightness");
    }

    /// An explicit mode passes straight through as u32.
    #[test]
    fn explicit_mode_passes_through() {
        let u = pack_uniform(&[ParamValue::Int(5)]);
        assert_eq!(u[0], 5, "mode Max");
    }
}
