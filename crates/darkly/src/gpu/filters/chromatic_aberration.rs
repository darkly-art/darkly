//! Chromatic aberration filter — a dynamic list of per-channel offset/scale/
//! color/blur "aberrations" summed into an RGB fringe. One registration serves
//! both effect surfaces the filter subsystem drives (destructive apply + filter
//! layer); the veil (`gpu/veils/chromatic_aberration.rs`) reuses this module's
//! [`PARAMS`], [`pack_uniform`], and [`GpuAberrationParams`].
//!
//! Per-pixel: the sample position for entry *i* is
//! `center + (p − center)·scale_i + offset_i`; the output rgb is the
//! color-weighted, `inv_sum`-normalized sum of the entries' blurred samples,
//! premultiplied to respect straight-alpha layers. The transform itself lives in
//! [`lib/aberration.wgsl`](../../../shaders/lib/aberration.wgsl); this module
//! declares the schema and packs the params into the shader's 528-byte uniform.
//!
//! Unlike the other parametric filters this one reads its source with
//! [`SrcSampling::Bilinear`] — the ghost/blur taps land on fractional offsets.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytemuck::Zeroable;

use crate::gpu::effect::EffectCache;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::param_filter::{ParamFilter, SrcSampling};
use crate::gpu::params::{ConstParamValue, ParamDef, ParamValue};

/// Uniform-array size (and the schema's entry cap). The UI disables "Add" at the
/// limit; [`pack_uniform`] still clamps defensively.
pub const MAX_ABERRATIONS: usize = 16;

const EPS: f32 = 1e-4;

/// Schema for a single aberration entry.
const ABERRATION_ITEM: &[ParamDef] = &[
    ParamDef::Vec2 {
        name: "offset",
        max: 64.0,
        default: [0.0, 0.0],
    },
    ParamDef::Float {
        name: "scale",
        min: 0.9,
        max: 1.1,
        default: 1.0,
    },
    ParamDef::Color {
        name: "color",
        default: [1.0, 1.0, 1.0],
    },
    ParamDef::Float {
        name: "blur",
        min: 0.0,
        // Max blur kept modest so a bounded tap count can't band.
        max: 6.0,
        default: 0.0,
    },
];

/// One `aberrations` list param with the photographic 3-entry default: red
/// scaled out (1.004), green identity, blue scaled in (0.996), each softened a
/// touch — the classic lens-fringe look out of the box.
pub const PARAMS: &[ParamDef] = &[ParamDef::List {
    name: "aberrations",
    item: ABERRATION_ITEM,
    max_len: MAX_ABERRATIONS,
    default: &[
        &[
            ("scale", ConstParamValue::Float(1.004)),
            ("color", ConstParamValue::Color([1.0, 0.0, 0.0])),
            ("blur", ConstParamValue::Float(0.6)),
        ],
        &[
            ("color", ConstParamValue::Color([0.0, 1.0, 0.0])),
            ("blur", ConstParamValue::Float(0.6)),
        ],
        &[
            ("scale", ConstParamValue::Float(0.996)),
            ("color", ConstParamValue::Color([0.0, 0.0, 1.0])),
            ("blur", ConstParamValue::Float(0.6)),
        ],
    ],
}];

/// One aberration in the shader's uniform (32 B). Field offsets match
/// `struct Aberration` in `lib/aberration.wgsl` (vec3 color at offset 16).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuAberration {
    pub offset_px: [f32; 2],
    pub scale: f32,
    pub blur_px: f32,
    pub color: [f32; 3],
    pub alpha_weight: f32,
}

/// The whole effect uniform (528 B): the channel-wise color normalizer, the live
/// entry count, and the fixed entry array. Matches `struct AberrationParams`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuAberrationParams {
    pub inv_sum: [f32; 3],
    pub count: u32,
    pub entries: [GpuAberration; MAX_ABERRATIONS],
}

/// Read a float from a list entry, tolerating an `Int` where a `Float` is
/// expected (whole floats degrade to `Int` on def-less document paths).
fn entry_float(entry: &BTreeMap<String, ParamValue>, key: &str, default: f32) -> f32 {
    match entry.get(key) {
        Some(ParamValue::Float(v)) => *v,
        Some(ParamValue::Int(v)) => *v as f32,
        _ => default,
    }
}

fn entry_vec2(entry: &BTreeMap<String, ParamValue>, key: &str) -> [f32; 2] {
    match entry.get(key) {
        Some(ParamValue::Vec2(v)) => *v,
        _ => [0.0, 0.0],
    }
}

fn entry_color(entry: &BTreeMap<String, ParamValue>, key: &str) -> [f32; 3] {
    match entry.get(key) {
        Some(ParamValue::Color(c)) => *c,
        _ => [1.0, 1.0, 1.0],
    }
}

/// Pack the `aberrations` list into the shader uniform. Offsets pass through
/// (already vectors); `inv_sum` is the channel-wise `1/max(Σ color, ε)` so
/// colors summing to white at identity transforms are exact passthroughs;
/// `alpha_weight` is each entry's mean color weight normalized to sum to 1.
///
/// Tolerant by design (the untagged-degradation posture): any non-`List` value
/// packs as the empty list (`count = 0` → shader passthrough), and `Int` is
/// accepted where a `Float` is expected inside entries.
pub fn pack_uniform(params: &[ParamValue]) -> GpuAberrationParams {
    let list: &[BTreeMap<String, ParamValue>] = match params.first() {
        Some(ParamValue::List(entries)) => entries,
        _ => &[],
    };

    let mut entries = [GpuAberration::zeroed(); MAX_ABERRATIONS];
    let count = list.len().min(MAX_ABERRATIONS);
    let mut color_sum = [0.0f32; 3];
    let mut mean_sum = 0.0f32;

    for (slot, entry) in entries.iter_mut().zip(list.iter()).take(MAX_ABERRATIONS) {
        let color = entry_color(entry, "color");
        let mean = (color[0] + color[1] + color[2]) / 3.0;
        color_sum[0] += color[0];
        color_sum[1] += color[1];
        color_sum[2] += color[2];
        mean_sum += mean;
        *slot = GpuAberration {
            offset_px: entry_vec2(entry, "offset"),
            scale: entry_float(entry, "scale", 1.0),
            blur_px: entry_float(entry, "blur", 0.0).max(0.0),
            color,
            alpha_weight: mean,
        };
    }

    // Normalize alpha weights so the output alpha is a proper weighted average.
    let inv_mean = 1.0 / mean_sum.max(EPS);
    for slot in entries.iter_mut().take(count) {
        slot.alpha_weight *= inv_mean;
    }

    GpuAberrationParams {
        inv_sum: [
            1.0 / color_sum[0].max(EPS),
            1.0 / color_sum[1].max(EPS),
            1.0 / color_sum[2].max(EPS),
        ],
        count: count as u32,
        entries,
    }
}

/// The CA fragment shader: the shared aberration lib prepended to the filter
/// shader (built at load time — the render shaders have no `#include`).
fn ca_shader_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../../../shaders/lib/aberration.wgsl"),
        include_str!("../../../shaders/filters/chromatic_aberration.wgsl"),
    )
}

/// Allocate (once) and refresh the 528-byte params uniform — the [`ParamFilter`]
/// `prepare` half.
fn ca_prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    params: &[ParamValue],
    cache: &mut EffectCache,
) {
    if cache.uniform_bufs.is_empty() {
        cache
            .uniform_bufs
            .push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("filter-chromatic-aberration-uniform"),
                size: std::mem::size_of::<GpuAberrationParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
    }
    let packed = pack_uniform(params);
    queue.write_buffer(&cache.uniform_bufs[0], 0, bytemuck::bytes_of(&packed));
}

fn create_pipeline(device: &wgpu::Device) -> Arc<dyn FilterEffect> {
    Arc::new(ParamFilter::new(
        device,
        "filter-chromatic-aberration",
        &ca_shader_source(),
        "fs_ca",
        "fs_ca_masked",
        false, // no aux texture — packed uniform only
        SrcSampling::Bilinear,
        ca_prepare,
    ))
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "chromatic_aberration",
        display_name: "Chromatic Aberration",
        params: PARAMS,
        create_pipeline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed_default() -> GpuAberrationParams {
        let params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        pack_uniform(&params)
    }

    /// The photographic default packs three entries with the red/green/blue
    /// scale spread and a channel-wise `inv_sum` of 1 (colors sum to white).
    #[test]
    fn photographic_defaults_pack() {
        let u = packed_default();
        assert_eq!(u.count, 3);
        assert_eq!(u.entries[0].scale, 1.004);
        assert_eq!(u.entries[1].scale, 1.0);
        assert_eq!(u.entries[2].scale, 0.996);
        assert_eq!(u.entries[0].color, [1.0, 0.0, 0.0]);
        assert_eq!(u.entries[1].color, [0.0, 1.0, 0.0]);
        assert_eq!(u.entries[2].color, [0.0, 0.0, 1.0]);
        // Colors sum to white → inv_sum is 1 per channel (exact passthrough).
        assert_eq!(u.inv_sum, [1.0, 1.0, 1.0]);
        // Mean weights (1/3 each) normalize to sum to 1.
        let a: f32 = u.entries[..3].iter().map(|e| e.alpha_weight).sum();
        assert!((a - 1.0).abs() < 1e-6, "alpha weights sum to 1, got {a}");
    }

    /// A single white, zero-offset, identity-scale entry → `inv_sum` exactly 1
    /// and `alpha_weight` exactly 1 (defends the ε-normalization identity path).
    #[test]
    fn white_identity_entry_is_exact() {
        let params = vec![ParamValue::List(vec![BTreeMap::from([
            ("offset".to_string(), ParamValue::Vec2([0.0, 0.0])),
            ("scale".to_string(), ParamValue::Float(1.0)),
            ("color".to_string(), ParamValue::Color([1.0, 1.0, 1.0])),
            ("blur".to_string(), ParamValue::Float(0.0)),
        ])])];
        let u = pack_uniform(&params);
        assert_eq!(u.count, 1);
        assert_eq!(u.inv_sum, [1.0, 1.0, 1.0]);
        assert_eq!(u.entries[0].alpha_weight, 1.0);
    }

    /// Offsets pass through unchanged (they're already vectors).
    #[test]
    fn offset_passes_through() {
        let params = vec![ParamValue::List(vec![BTreeMap::from([(
            "offset".to_string(),
            ParamValue::Vec2([4.0, -3.0]),
        )])])];
        let u = pack_uniform(&params);
        assert_eq!(u.entries[0].offset_px, [4.0, -3.0]);
    }

    /// More than `MAX_ABERRATIONS` entries clamp to the cap.
    #[test]
    fn caps_at_max() {
        let entry = BTreeMap::from([("color".to_string(), ParamValue::Color([1.0, 1.0, 1.0]))]);
        let params = vec![ParamValue::List(vec![entry; MAX_ABERRATIONS + 5])];
        let u = pack_uniform(&params);
        assert_eq!(u.count, MAX_ABERRATIONS as u32);
    }

    /// Empty list → count 0 (shader passthrough).
    #[test]
    fn empty_list_is_passthrough() {
        let u = pack_uniform(&[ParamValue::List(vec![])]);
        assert_eq!(u.count, 0);
    }

    /// A zero color sum is ε-guarded — `inv_sum` is finite, never a divide-by-0.
    #[test]
    fn zero_color_sum_is_guarded() {
        let params = vec![ParamValue::List(vec![BTreeMap::from([(
            "color".to_string(),
            ParamValue::Color([0.0, 0.0, 0.0]),
        )])])];
        let u = pack_uniform(&params);
        assert!(u.inv_sum.iter().all(|c| c.is_finite()));
    }

    /// Tolerant reads: a non-`List` value packs as the empty list (count 0), and
    /// an `Int` in a `Float` slot is accepted.
    #[test]
    fn tolerant_reads() {
        // Non-List degraded value (the benign `Curve([])` collision) → count 0.
        let u = pack_uniform(&[ParamValue::Curve(vec![])]);
        assert_eq!(u.count, 0);

        // Int where a Float scale is expected.
        let params = vec![ParamValue::List(vec![BTreeMap::from([(
            "scale".to_string(),
            ParamValue::Int(1),
        )])])];
        let u = pack_uniform(&params);
        assert_eq!(u.entries[0].scale, 1.0);
    }

    /// The uniform structs have the sizes the WGSL layout expects.
    #[test]
    fn uniform_sizes() {
        assert_eq!(std::mem::size_of::<GpuAberration>(), 32);
        assert_eq!(std::mem::size_of::<GpuAberrationParams>(), 528);
    }
}
