//! Tests for [`darkly::brush::warp_field`] — the displacement field that
//! backs liquify's scratch, and the single resample that turns it into
//! pixels.
//!
//! The load-bearing property is *exactness*. The resolve rewrites the
//! whole layer on every pen event, and almost every pixel of it has a
//! zero displacement. If the resolve were off by half a texel, every one
//! of those pixels would be softened on every commit — which is the very
//! defect the warp field exists to remove, reintroduced one layer down
//! and invisible to a contrast metric. So these assertions are
//! byte-identity, not tolerance: a tolerance would hide the bug.

use std::sync::{Arc, OnceLock};

use wgpu::util::DeviceExt;

use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::warp_field::{WarpFieldResolve, FIELD_FORMAT, RESOLVE_PIPELINE_ID};
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};

const W: u32 = 64;
const H: u32 = 64;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

/// Deterministic high-frequency source: every pixel differs from its
/// neighbours, so any interpolation error shows up as a changed byte.
fn busy_source() -> Vec<u8> {
    let mut out = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            out[i] = ((x * 7 + y * 13) % 256) as u8;
            out[i + 1] = ((x * 31) % 256) as u8;
            out[i + 2] = ((y * 17 + 3) % 256) as u8;
            out[i + 3] = 255;
        }
    }
    out
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// Upload a displacement field, resolve `source` through it onto a fresh
/// destination, and read the destination back.
fn resolve_through(field: &[[f32; 2]], source: &[u8]) -> Vec<u8> {
    let (device, queue) = shared_device();
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );

    let mut bytes = Vec::with_capacity(field.len() * 8);
    for texel in field {
        bytes.extend_from_slice(&texel[0].to_le_bytes());
        bytes.extend_from_slice(&texel[1].to_le_bytes());
    }
    let field_tex = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: Some("test-warp-field"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FIELD_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &bytes,
    );
    let field_view = field_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let (_src_tex, src_view) = create_test_texture(&device, &queue, W, H, source);

    // Destination starts as a distinct colour, so "the resolve wrote
    // nothing" cannot masquerade as a pass.
    let dest = vec![7u8; (W * H * 4) as usize];
    let (dest_tex, dest_view) = create_test_texture(&device, &queue, W, H, &dest);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("warp-field-test-resolve"),
    });
    pipelines
        .get::<WarpFieldResolve>(RESOLVE_PIPELINE_ID)
        .resolve(
            &device,
            &mut encoder,
            &field_view,
            &src_view,
            &dest_view,
            wgpu::TextureFormat::Rgba8Unorm,
            (W, H),
        );
    queue.submit([encoder.finish()]);

    readback_texture(
        &device,
        &queue,
        &dest_tex,
        wgpu::TextureFormat::Rgba8Unorm,
        W,
        H,
    )
}

/// **The texel-exactness guard.** A zero field must reproduce the source
/// byte for byte — not approximately.
///
/// Every pen event resolves the entire layer, and outside the brush disc
/// the field is exactly zero. A half-texel UV error here would low-pass
/// the whole image a little more on every commit: the ghosting bug again,
/// at a different layer, and a contrast metric would never see it.
#[test]
fn warp_field_resolve_is_identity_where_field_is_zero() {
    let source = busy_source();
    let zero_field = vec![[0.0_f32, 0.0]; (W * H) as usize];
    let out = resolve_through(&zero_field, &source);

    assert_eq!(
        out, source,
        "a zero displacement field must reproduce the source exactly; \
         any difference means the resolve's sampling is off by a \
         sub-texel amount and is quietly blurring the layer on every \
         commit",
    );
}

/// The same guarantee where it is hardest to keep: a field that is zero
/// almost everywhere but non-zero in a disc. The untouched majority must
/// still be byte-identical.
#[test]
fn warp_field_resolve_leaves_untouched_pixels_exact() {
    let source = busy_source();
    let centre = (32.0_f32, 32.0_f32);
    let radius = 10.0_f32;
    let mut field = vec![[0.0_f32, 0.0]; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let dx = x as f32 + 0.5 - centre.0;
            let dy = y as f32 + 0.5 - centre.1;
            if (dx * dx + dy * dy).sqrt() < radius {
                field[(y * W + x) as usize] = [-3.5, 2.25];
            }
        }
    }
    let out = resolve_through(&field, &source);

    let mut changed_outside = 0;
    let mut changed_inside = 0;
    for y in 0..H {
        for x in 0..W {
            let dx = x as f32 + 0.5 - centre.0;
            let dy = y as f32 + 0.5 - centre.1;
            let inside = (dx * dx + dy * dy).sqrt() < radius;
            let differs = pixel(&out, x, y) != pixel(&source, x, y);
            if differs && inside {
                changed_inside += 1;
            }
            if differs && !inside {
                changed_outside += 1;
            }
        }
    }
    assert_eq!(
        changed_outside, 0,
        "pixels with a zero field must be untouched; {changed_outside} \
         changed outside the displaced disc",
    );
    assert!(
        changed_inside > 100,
        "sanity: the displaced disc should have moved content \
         ({changed_inside} pixels changed inside it)",
    );
}

/// An integer displacement lands on texel centres, so a correct bilinear
/// fetch returns source texels verbatim — the output is a pure shifted
/// copy with **no** blended values anywhere.
///
/// This is what "the image is resampled once" means concretely: one
/// resample of an integer shift is lossless, whereas the per-dab image
/// warp this replaced would have produced interpolated mush.
#[test]
fn warp_field_resolve_is_single_resample() {
    let source = busy_source();
    let shift = [-4.0_f32, 6.0];
    let field = vec![shift; (W * H) as usize];
    let out = resolve_through(&field, &source);

    // Check away from the edges, where clamping legitimately differs.
    for y in 10..(H - 10) {
        for x in 10..(W - 10) {
            let sx = (x as f32 + shift[0]) as u32;
            let sy = (y as f32 + shift[1]) as u32;
            assert_eq!(
                pixel(&out, x, y),
                pixel(&source, sx, sy),
                "at ({x}, {y}): an integer displacement must copy source \
                 texel ({sx}, {sy}) verbatim — a blended value here means \
                 the fetch is misaligned",
            );
        }
    }
}
