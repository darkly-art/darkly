//! Curves filter — per-channel tone mapping via a baked 256×1 RGBA8 LUT.
//!
//! The first *parametric* filter. The user edits five independent curves — red,
//! green, blue, a composite "value", and alpha — on a 2D control-point editor;
//! each is a [`ParamValue::Curve`]. `ensure` evaluates them (through the shared
//! [`CurveLut`](crate::brush::curve_math::CurveLut) natural-cubic-spline, the
//! same Krita algorithm the brush curve node uses) into a lookup texture that
//! the fragment shader (`shaders/filters/curves.wgsl`) reads once per channel.
//!
//! Fold order follows GIMP's `gimp_curve_map_pixels`
//! (`gimp/app/core/gimpcurve-map.c:169-179`): the per-channel curve is applied
//! first, then the composite "value" curve on top — `dest = value(channel(src))`
//! — and the value curve is *not* applied to alpha. We bake exactly that into
//! the LUT so the shader stays a single `textureLoad` per channel.

use std::sync::Arc;

use crate::brush::curve_math::CurveLut;
use crate::gpu::effect::EffectCache;
use crate::gpu::filter::{FilterEffect, FilterPipelineRegistration};
use crate::gpu::params::{ParamDef, ParamValue};

/// Identity curve — a straight line through the two endpoints.
const IDENTITY: &[[f32; 2]] = &[[0.0, 0.0], [1.0, 1.0]];

/// Number of LUT entries (one per 8-bit input value).
const LUT_LEN: usize = 256;

/// Parameter schema. Order is load-bearing: [`build_lut`] indexes these
/// positionally, and the shader reads the baked components in the same order.
/// `value` is the composite curve applied on top of every color channel.
pub const PARAMS: &[ParamDef] = &[
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
        name: "value",
        default: IDENTITY,
    },
    ParamDef::Curve {
        name: "alpha",
        default: IDENTITY,
    },
];

/// Positional indices into [`PARAMS`] / a filter layer's param vector.
const RED: usize = 0;
const GREEN: usize = 1;
const BLUE: usize = 2;
const VALUE: usize = 3;
const ALPHA: usize = 4;

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

/// Bake the five curves into a 256×1 RGBA8 LUT (`256 * 4` bytes, row-major).
/// `lut.rgb[i] = value(channel(i/255))`, `lut.a[i] = alpha(i/255)` — the value
/// curve rides on top of each color channel but never on alpha.
fn build_lut(params: &[ParamValue]) -> [u8; LUT_LEN * 4] {
    let red = CurveLut::from_points(&curve_points(params, RED));
    let green = CurveLut::from_points(&curve_points(params, GREEN));
    let blue = CurveLut::from_points(&curve_points(params, BLUE));
    let value = CurveLut::from_points(&curve_points(params, VALUE));
    let alpha = CurveLut::from_points(&curve_points(params, ALPHA));

    let mut lut = [0u8; LUT_LEN * 4];
    for i in 0..LUT_LEN {
        let t = i as f32 / (LUT_LEN - 1) as f32;
        lut[i * 4] = to_u8(value.evaluate(red.evaluate(t)));
        lut[i * 4 + 1] = to_u8(value.evaluate(green.evaluate(t)));
        lut[i * 4 + 2] = to_u8(value.evaluate(blue.evaluate(t)));
        lut[i * 4 + 3] = to_u8(alpha.evaluate(t));
    }
    lut
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
/// `[src texture, lut texture]` (both `textureLoad`, no sampler). The per-layer
/// LUT texture lives in the compositor's [`EffectCache`], built by [`ensure`].
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
            entries: &[load_tex_entry(0), load_tex_entry(1)],
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
        // Allocate the LUT texture once; subsequent param edits reuse it.
        if cache.aux_textures.is_empty() {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("filter-curves-lut"),
                size: wgpu::Extent3d {
                    width: LUT_LEN as u32,
                    height: 1,
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

        let lut = build_lut(params);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &cache.aux_textures[0],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &lut,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((LUT_LEN * 4) as u32),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: LUT_LEN as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
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
        // so the LUT view is present. Guard defensively rather than panic in the
        // render loop.
        let Some(lut_view) = cache.aux_views.first() else {
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

    /// Identity curves ⇒ identity LUT: every entry maps its input value back to
    /// itself (`lut.c[i] == i`), so a pixel is bit-unchanged. This is the
    /// invariant the `textureLoad(round(v*255))` index convention relies on.
    #[test]
    fn identity_curves_yield_identity_lut() {
        let params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        let lut = build_lut(&params);
        for i in 0..LUT_LEN {
            for c in 0..4 {
                assert_eq!(
                    lut[i * 4 + c] as usize,
                    i,
                    "identity LUT entry {i} channel {c}"
                );
            }
        }
    }

    /// Fold order matches GIMP: the color channels are `value(channel(i))` — the
    /// per-channel curve first, then the composite value curve on top. Both
    /// curves are two-point (exact linear in `CurveLut`) so the arithmetic is
    /// unambiguous and no spline overshoot muddies the assertions.
    #[test]
    fn value_curve_composes_over_channel_curve() {
        // red(t) = 0.5·t (halve); value(x) = min(2·x, 1) (double, clamped).
        // Correct fold: value(red(t)) = min(t, 1) = t → identity.
        // Wrong fold red(value(t)) = 0.5·min(2t,1) would map the top to 0.5.
        let half = vec![[0.0, 0.0], [1.0, 0.5]];
        let double = vec![[0.0, 0.0], [0.5, 1.0]];
        let params = vec![
            ParamValue::Curve(half), // red
            ParamValue::Curve(IDENTITY.to_vec()),
            ParamValue::Curve(IDENTITY.to_vec()),
            ParamValue::Curve(double), // value
            ParamValue::Curve(IDENTITY.to_vec()),
        ];
        let lut = build_lut(&params);
        // value(red(1.0)) = min(2·0.5, 1) = 1.0 → 255. The wrong order gives 128.
        assert_eq!(lut[255 * 4], 255, "value(red(255)) must map back to 255");
        // Midpoint: value(red(0.5)) = min(2·0.25, 1) = 0.5 → ~128.
        let mid = lut[128 * 4];
        assert!(
            (mid as i32 - 128).abs() <= 2,
            "value(red(0.5)) ≈ 0.5, got {mid}"
        );
    }

    /// The value curve is applied to R/G/B but never to alpha (GIMP line 178):
    /// with an identity alpha curve and a non-identity value curve, the alpha
    /// column stays identity.
    #[test]
    fn value_curve_never_touches_alpha() {
        let double = vec![[0.0, 0.0], [0.5, 1.0], [1.0, 1.0]];
        let params = vec![
            ParamValue::Curve(IDENTITY.to_vec()),
            ParamValue::Curve(IDENTITY.to_vec()),
            ParamValue::Curve(IDENTITY.to_vec()),
            ParamValue::Curve(double), // value curve is aggressive
            ParamValue::Curve(IDENTITY.to_vec()), // alpha identity
        ];
        let lut = build_lut(&params);
        for i in 0..LUT_LEN {
            assert_eq!(
                lut[i * 4 + 3] as usize,
                i,
                "alpha must stay identity regardless of the value curve, entry {i}"
            );
        }
    }
}
