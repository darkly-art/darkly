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
//! Transform semantics, matching Krita:
//! - **RGB / Red / Green / Blue / Alpha** — per-component curves. Krita applies
//!   the per-channel curve first, then the composite "RGB" curve on top
//!   (`kis_multichannel_utils.cpp:256-263`, `colorTransform` then
//!   `allColorsTransform`), and the composite curve is *not* applied to alpha
//!   (`:241`). Both fold into row 0 of the LUT: `rgb[i] = rgb(channel(i))`,
//!   `a[i] = alpha(i)`.
//! - **Hue / Saturation** — HSV curves (`hsv_curve_adjustment`, non-relative:
//!   the curve replaces the channel value; `kis_hsv_adjustment.cpp:765-771`).
//!   The shader does one RGB→HSV→RGB round trip.
//! - **Lightness** — a curve on CIELAB L* (Krita's
//!   `createBrightnessContrastAdjustment` builds an L*a*b* device link on the L
//!   channel, `LcmsColorSpace.h:301`). The shader does an sRGB→Lab→sRGB round
//!   trip.
//!
//! The three 1D remap curves for Hue/Saturation/Lightness live in row 1 of the
//! LUT; the color-space conversions happen per pixel in `curves.wgsl`. The HSV
//! and Lab stages are gated by `_active` flags so an all-identity layer is a
//! byte-for-byte no-op (Krita likewise skips null transforms).

use std::sync::Arc;

use crate::brush::curve_math::CurveLut;
use crate::gpu::effect::EffectCache;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::params::{ParamDef, ParamValue};

/// Identity curve — a straight line through the two endpoints.
const IDENTITY: &[[f32; 2]] = &[[0.0, 0.0], [1.0, 1.0]];

/// Number of LUT entries per row (one per 8-bit input value).
const LUT_LEN: usize = 256;
/// The LUT is two rows: row 0 = per-component color curves, row 1 = HSL curves.
const LUT_ROWS: usize = 2;
/// Total LUT byte length (`256 × 2 × RGBA8`).
const LUT_BYTES: usize = LUT_LEN * LUT_ROWS * 4;

/// Parameter schema — Krita's channel order for an RGBA image. Load-bearing:
/// [`build_lut`] indexes these positionally and the shader reads baked
/// components in the same order.
pub const PARAMS: &[ParamDef] = &[
    ParamDef::Curve {
        name: "rgb",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "red",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "green",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "blue",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "alpha",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "hue",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "saturation",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "lightness",
        default: IDENTITY,
    },
];

/// Positional indices into [`PARAMS`] / a filter layer's param vector.
const RGB: usize = 0;
const RED: usize = 1;
const GREEN: usize = 2;
const BLUE: usize = 3;
const ALPHA: usize = 4;
const HUE: usize = 5;
const SATURATION: usize = 6;
const LIGHTNESS: usize = 7;

/// Read a curve param's control points by index, falling back to identity when
/// the param is missing or malformed (fewer than two points).
fn curve_points(params: &[ParamValue], idx: usize) -> Vec<[f32; 2]> {
    match params.get(idx) {
        Some(ParamValue::Curve(pts)) if pts.len() >= 2 => pts.clone(),
        _ => IDENTITY.to_vec(),
    }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// A baked curves LUT plus the stage-gating flags the shader reads.
struct Baked {
    /// 256×2 RGBA8, row-major. Row 0 = `[rgb∘red, rgb∘green, rgb∘blue, alpha]`;
    /// row 1 = `[hue, saturation, lightness, 0]`.
    lut: [u8; LUT_BYTES],
    /// Whether the Hue or Saturation curve is non-identity (gates the HSV pass).
    hsv_active: bool,
    /// Whether the Lightness curve is non-identity (gates the Lab pass).
    lightness_active: bool,
}

/// Evaluate `curve` across the 256 input steps into a u8 ramp.
fn ramp(curve: &CurveLut) -> [u8; LUT_LEN] {
    let mut out = [0u8; LUT_LEN];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = to_u8(curve.evaluate(i as f32 / (LUT_LEN - 1) as f32));
    }
    out
}

/// True when a u8 ramp maps every input to itself (an identity curve, up to
/// 8-bit rounding).
fn is_identity(ramp: &[u8; LUT_LEN]) -> bool {
    ramp.iter().enumerate().all(|(i, &v)| v as usize == i)
}

/// Bake the eight curves into the LUT + gate flags (see [`Baked`]).
fn build_lut(params: &[ParamValue]) -> Baked {
    let rgb = CurveLut::from_points(&curve_points(params, RGB));
    let red = CurveLut::from_points(&curve_points(params, RED));
    let green = CurveLut::from_points(&curve_points(params, GREEN));
    let blue = CurveLut::from_points(&curve_points(params, BLUE));
    let alpha = CurveLut::from_points(&curve_points(params, ALPHA));
    let hue = ramp(&CurveLut::from_points(&curve_points(params, HUE)));
    let sat = ramp(&CurveLut::from_points(&curve_points(params, SATURATION)));
    let light = ramp(&CurveLut::from_points(&curve_points(params, LIGHTNESS)));

    let mut lut = [0u8; LUT_BYTES];
    let row1 = LUT_LEN * 4;
    for i in 0..LUT_LEN {
        let t = i as f32 / (LUT_LEN - 1) as f32;
        // Row 0: per-channel curve first, then the composite RGB curve on top;
        // the composite curve is not applied to alpha.
        lut[i * 4] = to_u8(rgb.evaluate(red.evaluate(t)));
        lut[i * 4 + 1] = to_u8(rgb.evaluate(green.evaluate(t)));
        lut[i * 4 + 2] = to_u8(rgb.evaluate(blue.evaluate(t)));
        lut[i * 4 + 3] = to_u8(alpha.evaluate(t));
        // Row 1: the raw HSL remap curves.
        lut[row1 + i * 4] = hue[i];
        lut[row1 + i * 4 + 1] = sat[i];
        lut[row1 + i * 4 + 2] = light[i];
        lut[row1 + i * 4 + 3] = 0;
    }

    Baked {
        lut,
        hsv_active: !is_identity(&hue) || !is_identity(&sat),
        lightness_active: !is_identity(&light),
    }
}

/// A `textureLoad` source binding (no sampler / hardware filtering).
fn load_tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// GPU realization of the curves filter: one plain-RGBA pipeline that binds
/// `[src texture, lut texture, gate uniform]`. The per-layer LUT texture and
/// uniform live in the compositor's [`EffectCache`], built by [`ensure`].
///
/// [`ensure`]: FilterEffect::ensure
pub struct CurvesFilter {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

impl CurvesFilter {
    fn new(device: &wgpu::Device) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("filter-curves-bgl"),
            entries: &[
                load_tex_entry(0),
                load_tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("filter-curves-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("filter-curves-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/filters/curves.wgsl").into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("filter-curves"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_curves"),
                targets: &[Some(wgpu::ColorTargetState {
                    // Curves only serves the filter-*layer* compose path, which
                    // is RGBA8-only (the destructive/R8 path excludes parametric
                    // filters at the engine layer).
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        CurvesFilter { pipeline, bgl }
    }
}

impl std::fmt::Debug for CurvesFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurvesFilter").finish_non_exhaustive()
    }
}

impl FilterEffect for CurvesFilter {
    fn ensure(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &[ParamValue],
        cache: &mut EffectCache,
    ) {
        // Allocate the LUT texture + gate uniform once; param edits reuse them.
        if cache.aux_textures.is_empty() {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("filter-curves-lut"),
                size: wgpu::Extent3d {
                    width: LUT_LEN as u32,
                    height: LUT_ROWS as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            cache.aux_textures.push(tex);
            cache.aux_views.push(view);
        }
        if cache.uniform_bufs.is_empty() {
            cache
                .uniform_bufs
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("filter-curves-flags"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }

        let baked = build_lut(params);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &cache.aux_textures[0],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &baked.lut,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((LUT_LEN * 4) as u32),
                rows_per_image: Some(LUT_ROWS as u32),
            },
            wgpu::Extent3d {
                width: LUT_LEN as u32,
                height: LUT_ROWS as u32,
                depth_or_array_layers: 1,
            },
        );
        let flags = [
            baked.hsv_active as u32,
            baked.lightness_active as u32,
            0u32,
            0u32,
        ];
        queue.write_buffer(&cache.uniform_bufs[0], 0, bytemuck::cast_slice(&flags));
    }

    fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        _mask: Option<&wgpu::TextureView>,
        out: &wgpu::TextureView,
        _format: wgpu::TextureFormat,
        cache: &EffectCache,
    ) {
        // The compose path always runs `ensure` first (pre-compose sync phase),
        // so the LUT view + uniform are present. Guard defensively rather than
        // panic in the render loop.
        let (Some(lut_view), Some(uniform)) = (cache.aux_views.first(), cache.uniform_bufs.first())
        else {
            return;
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("filter-curves-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("filter-curves-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn create_pipeline(device: &wgpu::Device) -> Arc<dyn FilterEffect> {
    Arc::new(CurvesFilter::new(device))
}

pub fn register() -> FilterPipelineRegistration {
    FilterPipelineRegistration {
        type_id: "curves",
        display_name: "Curves",
        params: PARAMS,
        create_pipeline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
