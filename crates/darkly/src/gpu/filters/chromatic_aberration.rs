//! Chromatic aberration filter — a dynamic list of offset/scale/color/blur
//! "aberrations", each displacing the component of the image along its color's
//! hue axis over an otherwise-untouched base. One registration serves both
//! effect surfaces the filter subsystem drives (destructive apply + filter
//! layer); the veil (`gpu/veils/chromatic_aberration.rs`) reuses this module's
//! [`PARAMS`], [`pack_uniform`], and [`GpuAberrationParams`].
//!
//! Per entry the sample position for pixel *p* is
//! `center + (p − center)·scale_i + offset_i`; the displaced sample's
//! premultiplied delta from the base is split by [`analyze_color`] into an
//! achromatic full-pixel shift (`k1`) and a chromatic shift along the color's
//! hue `axis` (`k2`). "How much of the entry's hue is in this pixel" is answered
//! by rotating RGB about the gray diagonal so the hue lands on the red axis —
//! a smooth perceptual falloff with hue distance, not per-channel masking. The
//! transform itself lives in
//! [`lib/aberration.wgsl`](../../../shaders/lib/aberration.wgsl); this module
//! declares the schema and packs the params into the shader's 784-byte uniform.
//!
//! Unlike the other parametric filters this one reads its source with
//! [`SrcSampling::Bilinear`] — the ghost/blur taps land on fractional offsets.

use crate::units::UnitType;
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

/// Schema for a single aberration entry.
const ABERRATION_ITEM: &[ParamDef] = &[
    ParamDef::vec2("offset", 64.0, [0.0, 0.0])
        .with_label("Offset")
        .with_description(
            "How far this fringe is displaced from the original, and in which direction.",
        )
        .with_unit(UnitType::Pixels),
    ParamDef::float("scale", 0.9, 1.1, 1.0)
        .with_label("Scale")
        .with_description("Magnification of this fringe — values below 1 pull it inward."),
    ParamDef::color("color", [1.0, 1.0, 1.0])
        .with_label("Colour")
        .with_description("Which colour this fringe contributes."),
    ParamDef::float("blur", 0.0, 6.0, 0.0)
        .with_label("Blur")
        .with_description("Softens this fringe so it reads as defocus rather than a hard copy.")
        .with_unit(UnitType::Pixels),
];

/// One `aberrations` list param with the photographic 3-entry default: red
/// holds at unit magnification while green and blue shrink progressively inward
/// (1.00 / 0.99 / 0.98), a 1% step per channel — the wavelength-dependent focus
/// of a real lens fringing the shorter wavelengths inward. Each is softened a
/// touch.
pub const PARAMS: &[ParamDef] = &[ParamDef::list(
    "aberrations",
    ABERRATION_ITEM,
    MAX_ABERRATIONS,
    &[
        &[
            ("scale", ConstParamValue::Float(1.0)),
            ("color", ConstParamValue::Color([1.0, 0.0, 0.0])),
            ("blur", ConstParamValue::Float(0.6)),
        ],
        &[
            ("scale", ConstParamValue::Float(0.99)),
            ("color", ConstParamValue::Color([0.0, 1.0, 0.0])),
            ("blur", ConstParamValue::Float(0.6)),
        ],
        &[
            ("scale", ConstParamValue::Float(0.98)),
            ("color", ConstParamValue::Color([0.0, 0.0, 1.0])),
            ("blur", ConstParamValue::Float(0.6)),
        ],
    ],
)
.with_label("Fringes")
.with_description("The coloured copies the lens splits the image into.")];

/// One aberration in the shader's uniform (48 B). Field offsets match
/// `struct Aberration` in `lib/aberration.wgsl` (vec3 `axis` at offset 16). The
/// `_pad` rounds the struct to the vec3's 16-byte alignment for bytemuck.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuAberration {
    pub offset_px: [f32; 2],
    pub scale: f32,
    pub blur_px: f32,
    pub axis: [f32; 3],
    pub k1: f32,
    pub k2: f32,
    pub _pad: [f32; 3],
}

/// The whole effect uniform (784 B): the live entry count (padded to the entry
/// array's 16-byte alignment) and the fixed entry array. Matches
/// `struct AberrationParams`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuAberrationParams {
    pub count: u32,
    pub _pad: [u32; 3],
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

/// Analyze an entry color into its hue-rotation `axis` and the achromatic/
/// chromatic strength split (`k1`, `k2`) the shader applies to the displaced
/// content delta. Strength is `m = max(color)`; HSV saturation `s` splits it
/// into `k1 = m·(1−s)` (achromatic — a full-pixel shift) and `k2 = m·s`
/// (chromatic — a shift along the hue axis), so `k1 + k2 = m` always.
///
/// The axis is the red axis rotated by the color's hue `θ` about the gray
/// diagonal (Rodrigues): `cosθ·(1,0,0) + (sinθ/√3)·(0,1,−1) + ((1−cosθ)/3)·(1,1,1)`.
/// Its components sum to 1 (white analyzes to a full unit of any hue); at θ = 0,
/// ±120°, ±240° it lands on an exact channel unit vector (classic channel split).
/// A near-black color is a no-op (both scalars zero).
fn analyze_color(color: [f32; 3]) -> ([f32; 3], f32, f32) {
    let m = color[0].max(color[1]).max(color[2]);
    if m < 1e-3 {
        return ([1.0, 0.0, 0.0], 0.0, 0.0);
    }
    let min_c = color[0].min(color[1]).min(color[2]);
    let delta = m - min_c;
    let s = delta / m;

    // HSV hue in radians: 0 at red, 2π/3 at green, 4π/3 at blue.
    let theta = if delta < 1e-6 {
        0.0
    } else if m == color[0] {
        (((color[1] - color[2]) / delta).rem_euclid(6.0)) * std::f32::consts::FRAC_PI_3
    } else if m == color[1] {
        ((color[2] - color[0]) / delta + 2.0) * std::f32::consts::FRAC_PI_3
    } else {
        ((color[0] - color[1]) / delta + 4.0) * std::f32::consts::FRAC_PI_3
    };

    let (sin, cos) = theta.sin_cos();
    let a = sin / 3.0f32.sqrt();
    let b = (1.0 - cos) / 3.0;
    let axis = [cos + b, a + b, -a + b];
    (axis, m * (1.0 - s), m * s)
}

/// Pack the `aberrations` list into the shader uniform. Offsets/scale/blur pass
/// through; each entry's color is analyzed by [`analyze_color`] into the hue
/// `axis` and the `k1`/`k2` strength split the shader applies to the displaced
/// content delta.
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

    for (slot, entry) in entries.iter_mut().zip(list.iter()).take(MAX_ABERRATIONS) {
        let (axis, k1, k2) = analyze_color(entry_color(entry, "color"));
        *slot = GpuAberration {
            offset_px: entry_vec2(entry, "offset"),
            scale: entry_float(entry, "scale", 1.0),
            blur_px: entry_float(entry, "blur", 0.0).max(0.0),
            axis,
            k1,
            k2,
            _pad: [0.0; 3],
        };
    }

    GpuAberrationParams {
        count: count as u32,
        _pad: [0; 3],
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

/// Allocate (once) and refresh the 784-byte params uniform — the [`ParamFilter`]
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

/// Shared by the filter and veil registrations — like `PARAMS`, the veil
/// module imports it so both surfaces present one identity.
pub const DESCRIPTION: &str =
    "Split the color channels apart along their hue axes, like a misaligned lens.";

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "chromatic_aberration",
        display_name: "Chromatic Aberration",
        icon: "lucide-lab:venn",
        description: DESCRIPTION,
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

    fn approx(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6)
    }

    /// The photographic R/G/B default packs three fully-saturated primaries:
    /// each is a pure chromatic shift (`k1 = 0`, `k2 = 1`) along its channel's
    /// exact unit axis — the classic channel split.
    #[test]
    fn photographic_defaults_pack() {
        let u = packed_default();
        assert_eq!(u.count, 3);
        assert_eq!(u.entries[0].scale, 1.0);
        assert_eq!(u.entries[1].scale, 0.99);
        assert_eq!(u.entries[2].scale, 0.98);
        for (i, axis) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            assert!(
                approx(u.entries[i].axis, axis),
                "entry {i} axis {:?}",
                u.entries[i].axis
            );
            assert_eq!(
                u.entries[i].k1, 0.0,
                "primary entry {i} is purely chromatic"
            );
            assert!(
                (u.entries[i].k2 - 1.0).abs() < 1e-6,
                "entry {i} k2 {}",
                u.entries[i].k2
            );
        }
    }

    /// A white entry is purely achromatic: `k1 = 1`, `k2 = 0` (it shifts the
    /// whole pixel — the full-image-shift behavior).
    #[test]
    fn white_entry_is_full_shift() {
        let (axis, k1, k2) = analyze_color([1.0, 1.0, 1.0]);
        assert_eq!(k1, 1.0);
        assert_eq!(k2, 0.0);
        // Axis is unused when k2 = 0, but must stay finite.
        assert!(axis.iter().all(|c| c.is_finite()));
    }

    /// A dim (unsaturated-value) color scales the strength split by `m = max`:
    /// `k1 + k2 == m`, and the hue axis matches the same color at full value.
    #[test]
    fn dim_color_scales_strength_by_max() {
        let (dim_axis, k1, k2) = analyze_color([0.5, 0.0, 0.0]);
        assert!(
            (k1 + k2 - 0.5).abs() < 1e-6,
            "k1+k2 == max, got {}",
            k1 + k2
        );
        // Dim red is fully saturated → all chromatic.
        assert_eq!(k1, 0.0);
        assert!((k2 - 0.5).abs() < 1e-6);
        let (full_axis, _, _) = analyze_color([1.0, 0.0, 0.0]);
        assert!(approx(dim_axis, full_axis), "hue axis is value-independent");
    }

    /// A near-black color is a no-op: both strength scalars are zero, so the
    /// entry contributes nothing.
    #[test]
    fn near_black_color_is_no_op() {
        let (_, k1, k2) = analyze_color([0.0005, 0.0002, 0.0]);
        assert_eq!(k1, 0.0);
        assert_eq!(k2, 0.0);
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
        assert_eq!(std::mem::size_of::<GpuAberration>(), 48);
        assert_eq!(std::mem::size_of::<GpuAberrationParams>(), 784);
    }
}
