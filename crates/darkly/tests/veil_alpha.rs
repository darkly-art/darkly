//! Veils must carry the source's alpha rather than stamping the output opaque.
//!
//! Every veil used to end in `vec4f(colour, 1.0)`. That was invisible in the
//! viewport, whose input alpha is already 1.0 (`fs_present` returns
//! `vec4f(composed, 1.0)`), but it makes a veil unusable over canvas content:
//! anything it touched would be forced opaque, destroying transparency beneath
//! it and filling unpainted areas with colour.
//!
//! The contract pinned here is that alpha is **coverage** — averaged linearly
//! over whatever footprint the veil reads. Feeding a uniform half-transparent
//! source therefore has to come back half-transparent, because the linear mean
//! of a constant is that constant. A veil that hard-codes its alpha returns 255
//! and fails; one that zeroes alpha returns 0 and fails too.
//!
//! Run with:
//! `cargo test -p darkly --test veil_alpha --features testing -- --test-threads=1`

use darkly::gpu::effect::EffectRegistry;
use darkly::gpu::params::ParamDef;
use darkly::gpu::test_utils::*;

const W: u32 = 64;
const H: u32 = 64;
const SRC_ALPHA: u8 = 128;

/// A uniform RGBA source: mid-grey at half coverage. Uniform on purpose — a
/// spatial veil's footprint then cannot reach a differently-covered texel, so
/// any deviation in the result is the veil's own doing rather than its blur
/// kernel averaging in a neighbour.
fn half_covered_source() -> Vec<u8> {
    [90u8, 140, 200, SRC_ALPHA]
        .into_iter()
        .cycle()
        .take((W * H * 4) as usize)
        .collect()
}

fn render_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("veil-alpha-dst"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

/// Run one veil at its schema defaults over `half_covered_source()` and return
/// the alpha of the centre texel. Centre rather than a corner: `vhs` invents an
/// opaque off-tape letterbox for samples that fall outside the image, which is
/// authored content and legitimately opaque, and only edge fragments reach it.
fn veil_centre_alpha(type_id: &str) -> u8 {
    let (device, queue) = test_device();
    let format = wgpu::TextureFormat::Rgba8Unorm;

    let src = half_covered_source();
    let (_t0, v0) = create_test_texture(&device, &queue, W, H, &src);
    let (_t1, v1) = create_test_texture(&device, &queue, W, H, &src);
    let ping_pong = [v0, v1];

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("veil-alpha-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let mut registry = EffectRegistry::new();
    let params: Vec<_> = registry
        .params(type_id)
        .iter()
        .map(ParamDef::default_value)
        .collect();
    let mut veil = registry
        .instance(type_id, &params, &device, format)
        .expect("registered effect");
    let cache = veil.create_cache(&device, &queue, &ping_pong, &sampler, W, H);

    let (dst, dst_view) = render_target(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("veil-alpha-encode"),
    });
    veil.encode(&mut encoder, &cache, 0, &dst_view);
    queue.submit([encoder.finish()]);

    let out = readback_texture(&device, &queue, &dst, format, W, H);
    let centre = (((H / 2) * W + (W / 2)) * 4) as usize;
    out[centre + 3]
}

/// Every veil that reads the source must hand back the coverage it read.
///
/// The tolerance absorbs unorm round-tripping and the bilinear taps a spatial
/// veil makes across a uniform field; it is far tighter than the ±127 an
/// opaque-stamping veil would miss by.
#[test]
fn veils_carry_source_alpha() {
    for type_id in [
        "frozen",
        "grain",
        "lens_blur",
        "painting",
        "rainy_glass",
        "vhs",
        "black_and_white",
        "chromatic_aberration",
        "pixelate",
    ] {
        let got = veil_centre_alpha(type_id);
        let delta = (got as i16 - SRC_ALPHA as i16).abs();
        assert!(
            delta <= 2,
            "veil `{type_id}` returned alpha {got}, expected ~{SRC_ALPHA} \
             (delta {delta}) — it is not carrying the source's coverage",
        );
    }
}

/// A veil over fully transparent input must stay fully transparent. Pins the
/// other end of the range: `veils_carry_source_alpha` alone would pass for a
/// veil that scaled alpha by some constant that happens to land near 128.
#[test]
fn veils_leave_empty_canvas_empty() {
    for type_id in ["frozen", "lens_blur", "painting", "rainy_glass"] {
        let alpha = veil_over_transparent(type_id);
        assert_eq!(
            alpha, 0,
            "veil `{type_id}` returned alpha {alpha} over fully transparent \
             input — it would opaque-ify empty canvas",
        );
    }
}

/// As [`veil_centre_alpha`], but over a source with zero coverage everywhere.
fn veil_over_transparent(type_id: &str) -> u8 {
    let (device, queue) = test_device();
    let format = wgpu::TextureFormat::Rgba8Unorm;

    let src = vec![0u8; (W * H * 4) as usize];
    let (_t0, v0) = create_test_texture(&device, &queue, W, H, &src);
    let (_t1, v1) = create_test_texture(&device, &queue, W, H, &src);
    let ping_pong = [v0, v1];

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("veil-alpha-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let mut registry = EffectRegistry::new();
    let params: Vec<_> = registry
        .params(type_id)
        .iter()
        .map(ParamDef::default_value)
        .collect();
    let mut veil = registry
        .instance(type_id, &params, &device, format)
        .expect("registered effect");
    let cache = veil.create_cache(&device, &queue, &ping_pong, &sampler, W, H);

    let (dst, dst_view) = render_target(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("veil-alpha-encode"),
    });
    veil.encode(&mut encoder, &cache, 0, &dst_view);
    queue.submit([encoder.finish()]);

    let out = readback_texture(&device, &queue, &dst, format, W, H);
    let centre = (((H / 2) * W + (W / 2)) * 4) as usize;
    out[centre + 3]
}
