//! Transform and paste GPU integration tests: translation, rotation, paste, affine math.
//!
//! Tests TransformPass::commit_to_texture() with various transforms and undo.
//! Run with: `cargo test -p darkly --test transform`

use darkly::gpu::test_utils::*;
use darkly::gpu::region_store::RegionStore;
use darkly::gpu::transform::{
    TransformPass, Affine2D, IDENTITY,
    affine_translate, affine_inverse, affine_multiply,
};

fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") })
}

fn submit(queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) {
    queue.submit([encoder.finish()]);
}

fn pixel_at(pixels: &[u8], w: u32, x: u32, y: u32, bpp: u32) -> &[u8] {
    let offset = ((y * w + x) * bpp) as usize;
    &pixels[offset..offset + bpp as usize]
}

/// Helper: create a TransformPass and the dummy accumulator/cache textures
/// needed by set_floating_content. Returns (pass, accum_views, cache_view, sampler).
fn setup_transform_pass(
    device: &wgpu::Device,
    _queue: &wgpu::Queue,
    canvas_w: u32,
    canvas_h: u32,
) -> (
    TransformPass,
    [wgpu::TextureView; 2],
    wgpu::TextureView,
    wgpu::Sampler,
) {
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let pass = TransformPass::new(device, fmt);

    // Dummy accumulator textures (needed by set_floating_content for preview bind groups).
    let make_dummy = || {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dummy-accum"),
            size: wgpu::Extent3d { width: canvas_w, height: canvas_h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        tex.create_view(&wgpu::TextureViewDescriptor::default())
    };

    let accum_views = [make_dummy(), make_dummy()];
    let cache_view = make_dummy();

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    (pass, accum_views, cache_view, sampler)
}

/// Helper: create flat RGBA pixel data with a solid-color rectangle.
/// Returns (rgba_data, origin, width, height).
fn make_source_rect(
    x: i32, y: i32, w: u32, h: u32, color: [u8; 4],
) -> (Vec<u8>, (i32, i32), u32, u32) {
    let mut data = vec![0u8; (w * h * 4) as usize];
    for py in 0..h {
        for px in 0..w {
            let off = ((py * w + px) * 4) as usize;
            data[off..off + 4].copy_from_slice(&color);
        }
    }
    (data, (x, y), w, h)
}

// ============================================================================
// Transform commit with translation
// ============================================================================

/// Paint a 4×4 red block, translate by (10, 10), commit to texture.
/// Verify: red pixels at new position, old position is clear.
#[test]
fn transform_commit_translate() {
    let (device, queue) = test_device();
    let (cw, ch) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Target layer texture (transparent).
    let (target_tex, target_view) =
        create_test_texture(&device, &queue, cw, ch, &vec![0u8; (cw * ch * 4) as usize]);

    let (mut pass, accum_views, cache_view, sampler) =
        setup_transform_pass(&device, &queue, cw, ch);

    // Source: 4×4 red block at (10, 10).
    let (source_data, origin, sw, sh) = make_source_rect(10, 10, 4, 4, [255, 0, 0, 255]);

    pass.set_floating_content(
        &device, &queue, &sampler,
        &accum_views, &cache_view,
        &source_data, origin, sw, sh, cw, ch,
        1, false,
    );

    // Translate by (10, 10).
    let matrix = affine_translate(10.0, 10.0);

    let mut enc = encoder(&device);
    pass.commit_to_texture(
        &device, &mut enc, &queue, &target_tex, &target_view, fmt,
        &matrix, origin, sw, sh, cw, ch,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // New position: (20, 20) to (23, 23) should be red.
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let p = pixel_at(&pixels, cw, 20 + dx, 20 + dy, 4);
            assert!(p[0] > 200, "pixel at ({},{}) should be red, got R={}", 20 + dx, 20 + dy, p[0]);
            assert!(p[3] > 200, "pixel at ({},{}) should be opaque, got A={}", 20 + dx, 20 + dy, p[3]);
        }
    }

    // Old position: (10, 10) to (13, 13) should be transparent (target was blank,
    // commit only writes to new position).
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let p = pixel_at(&pixels, cw, 10 + dx, 10 + dy, 4);
            assert_eq!(p[3], 0, "old position ({},{}) should be transparent, A={}", 10 + dx, 10 + dy, p[3]);
        }
    }

    // Some unrelated pixel should be transparent.
    assert_eq!(pixel_at(&pixels, cw, 0, 0, 4)[3], 0);
}

/// Transform commit with translation + undo via RegionStore.
#[test]
fn transform_commit_translate_undo() {
    let (device, queue) = test_device();
    let (cw, ch) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (target_tex, target_view) =
        create_test_texture(&device, &queue, cw, ch, &vec![0u8; (cw * ch * 4) as usize]);
    let mut store = RegionStore::with_capacity(&device, cw, ch, 2 * 1024 * 1024);

    let (mut pass, accum_views, cache_view, sampler) =
        setup_transform_pass(&device, &queue, cw, ch);

    let (source_data, origin, sw, sh) = make_source_rect(5, 5, 4, 4, [0, 255, 0, 255]);

    pass.set_floating_content(
        &device, &queue, &sampler,
        &accum_views, &cache_view,
        &source_data, origin, sw, sh, cw, ch,
        1, false,
    );

    // Save pre-commit state.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &target_tex, fmt, [0, 0, cw, ch]);
    submit(&queue, enc);

    // Commit with translation (15, 15).
    let matrix = affine_translate(15.0, 15.0);
    let mut enc = encoder(&device);
    pass.commit_to_texture(&device, &mut enc, &queue, &target_tex, &target_view, fmt, &matrix, origin, sw, sh, cw, ch);
    submit(&queue, enc);

    // Commit undo entry.
    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, cw, ch]);
    submit(&queue, enc);

    // Verify green at new position.
    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    let p = pixel_at(&pixels, cw, 20, 20, 4);
    assert!(p[1] > 200, "should be green at new pos, G={}", p[1]);

    // Undo.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &target_tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    assert_eq!(pixel_at(&pixels, cw, 20, 20, 4)[3], 0, "after undo, should be transparent");
    assert_eq!(pixel_at(&pixels, cw, 5, 5, 4)[3], 0, "after undo, original pos still transparent");
}

// ============================================================================
// Transform commit with rotation
// ============================================================================

/// Paint a vertical line (x=2, y=0..4), rotate 90° CW, commit.
/// After rotation the line should be horizontal.
#[test]
fn transform_commit_rotate_90() {
    let (device, queue) = test_device();
    let (cw, ch) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (target_tex, target_view) =
        create_test_texture(&device, &queue, cw, ch, &vec![0u8; (cw * ch * 4) as usize]);

    let (mut pass, accum_views, cache_view, sampler) =
        setup_transform_pass(&device, &queue, cw, ch);

    // Source: vertical line at x=2 in a 5×5 block.
    let (sw, sh) = (5u32, 5u32);
    let (ox, oy) = (10i32, 10i32);
    let mut source_data = vec![0u8; (sw * sh * 4) as usize];
    for py in 0..5u32 {
        let off = ((py * sw + 2) * 4) as usize;
        source_data[off..off + 4].copy_from_slice(&[0, 0, 255, 255]);
    }

    pass.set_floating_content(
        &device, &queue, &sampler,
        &accum_views, &cache_view,
        &source_data, (ox, oy), sw, sh, cw, ch,
        1, false,
    );

    // Rotate 90° CW: matrix [0, 1, 0, -1, 0, 5]
    let matrix: Affine2D = [0.0, 1.0, 0.0, -1.0, 0.0, 5.0];

    let mut enc = encoder(&device);
    pass.commit_to_texture(&device, &mut enc, &queue, &target_tex, &target_view, fmt, &matrix, (ox, oy), sw, sh, cw, ch);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // Horizontal line at y=12, x=10..14.
    for x in 10..15u32 {
        let p = pixel_at(&pixels, cw, x, 12, 4);
        assert!(p[2] > 200, "rotated line at ({},12) should be blue, B={}", x, p[2]);
        assert!(p[3] > 200, "rotated line at ({},12) should be opaque, A={}", x, p[3]);
    }

    // Original vertical line position (x=12, y=10..11) should be transparent.
    let p = pixel_at(&pixels, cw, 12, 10, 4);
    assert_eq!(p[3], 0, "original vert line pos (12,10) should be clear, A={}", p[3]);
    let p = pixel_at(&pixels, cw, 12, 11, 4);
    assert_eq!(p[3], 0, "original vert line pos (12,11) should be clear, A={}", p[3]);
}

// ============================================================================
// Paste commit (identity transform)
// ============================================================================

/// Simulate paste: upload source image, commit with identity matrix onto a layer.
/// Verify pixels are composited correctly.
#[test]
fn paste_commit_identity() {
    let (device, queue) = test_device();
    let (cw, ch) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Target starts transparent.
    let (target_tex, target_view) =
        create_test_texture(&device, &queue, cw, ch, &vec![0u8; (cw * ch * 4) as usize]);

    let (mut pass, accum_views, cache_view, sampler) =
        setup_transform_pass(&device, &queue, cw, ch);

    // Source: 8×8 magenta block at (20, 20).
    let (source_data, origin, sw, sh) =
        make_source_rect(20, 20, 8, 8, [255, 0, 255, 255]);

    pass.set_floating_content(
        &device, &queue, &sampler,
        &accum_views, &cache_view,
        &source_data, origin, sw, sh, cw, ch,
        1, false,
    );

    // Commit with identity — pixels land at their original position.
    let mut enc = encoder(&device);
    pass.commit_to_texture(&device, &mut enc, &queue, &target_tex, &target_view, fmt, &IDENTITY, origin, sw, sh, cw, ch);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // Center of pasted block.
    let p = pixel_at(&pixels, cw, 24, 24, 4);
    assert!(p[0] > 200 && p[2] > 200, "should be magenta, got [{},{},{},{}]", p[0], p[1], p[2], p[3]);
    assert!(p[3] > 200, "should be opaque");

    // Outside the block.
    assert_eq!(pixel_at(&pixels, cw, 0, 0, 4)[3], 0, "outside should be transparent");
    assert_eq!(pixel_at(&pixels, cw, 19, 20, 4)[3], 0, "just outside should be transparent");
    assert_eq!(pixel_at(&pixels, cw, 28, 20, 4)[3], 0, "just outside right should be transparent");
}

/// Paste commit with undo round-trip.
#[test]
fn paste_commit_undo() {
    let (device, queue) = test_device();
    let (cw, ch) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (target_tex, target_view) =
        create_test_texture(&device, &queue, cw, ch, &vec![0u8; (cw * ch * 4) as usize]);
    let mut store = RegionStore::with_capacity(&device, cw, ch, 2 * 1024 * 1024);

    let (mut pass, accum_views, cache_view, sampler) =
        setup_transform_pass(&device, &queue, cw, ch);

    let (source_data, origin, sw, sh) =
        make_source_rect(10, 10, 6, 6, [255, 255, 0, 255]);

    pass.set_floating_content(
        &device, &queue, &sampler,
        &accum_views, &cache_view,
        &source_data, origin, sw, sh, cw, ch,
        1, false,
    );

    // Save pre-paste state.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &target_tex, fmt, [0, 0, cw, ch]);
    submit(&queue, enc);

    // Commit paste.
    let mut enc = encoder(&device);
    pass.commit_to_texture(&device, &mut enc, &queue, &target_tex, &target_view, fmt, &IDENTITY, origin, sw, sh, cw, ch);
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, cw, ch]);
    submit(&queue, enc);

    // Verify yellow pixels.
    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    let p = pixel_at(&pixels, cw, 12, 12, 4);
    assert!(p[0] > 200 && p[1] > 200, "should be yellow, got [{},{},{},{}]", p[0], p[1], p[2], p[3]);

    // Undo.
    let mut enc = encoder(&device);
    let forward = store.restore_region(&mut enc, &entry, &target_tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    assert_eq!(pixel_at(&pixels, cw, 12, 12, 4)[3], 0, "after undo, should be transparent");

    // Redo.
    let mut enc = encoder(&device);
    let _backward = store.restore_region(&mut enc, &forward, &target_tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    let p = pixel_at(&pixels, cw, 12, 12, 4);
    assert!(p[0] > 200, "after redo, should be yellow again, R={}", p[0]);
}

// ============================================================================
// Commit composites onto existing content (source-over blend)
// ============================================================================

/// Commit a semi-transparent source onto a solid background — verify blending.
#[test]
fn commit_composites_over_existing() {
    let (device, queue) = test_device();
    let (cw, ch) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Target: solid blue background.
    let blue: Vec<u8> = (0..cw * ch).flat_map(|_| [0u8, 0, 255, 255]).collect();
    let (target_tex, target_view) = create_test_texture(&device, &queue, cw, ch, &blue);

    let (mut pass, accum_views, cache_view, sampler) =
        setup_transform_pass(&device, &queue, cw, ch);

    // Source: semi-transparent red (alpha=128) at (10,10) size 4×4.
    let (source_data, origin, sw, sh) =
        make_source_rect(10, 10, 4, 4, [255, 0, 0, 128]);

    pass.set_floating_content(
        &device, &queue, &sampler,
        &accum_views, &cache_view,
        &source_data, origin, sw, sh, cw, ch,
        1, false,
    );

    let mut enc = encoder(&device);
    pass.commit_to_texture(&device, &mut enc, &queue, &target_tex, &target_view, fmt, &IDENTITY, origin, sw, sh, cw, ch);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // At (12, 12): semi-transparent red over blue.
    let p = pixel_at(&pixels, cw, 12, 12, 4);
    assert!(p[0] > 100 && p[0] < 180, "blended R should be ~128, got {}", p[0]);
    assert!(p[2] > 80 && p[2] < 180, "blended B should be ~127, got {}", p[2]);
    assert_eq!(p[3], 255, "result alpha should be 255 (fully opaque bg)");

    // Outside the source rect, blue should be untouched.
    let p = pixel_at(&pixels, cw, 0, 0, 4);
    assert_eq!(p, &[0, 0, 255, 255], "outside should still be blue");
}

// ============================================================================
// Transform commit on R8 mask target
// ============================================================================

/// Commit a white source block onto an R8 mask texture via transform commit.
#[test]
fn transform_commit_on_mask() {
    let (device, queue) = test_device();
    let (cw, ch) = (64, 64);
    let mask_fmt = wgpu::TextureFormat::R8Unorm;

    // Target: fully transparent mask (0).
    let (target_tex, target_view) =
        create_test_texture_with_format(&device, &queue, cw, ch, &vec![0u8; (cw * ch) as usize], mask_fmt);

    // TransformPass still uses Rgba8Unorm for its accumulator format (preview path).
    let (mut pass, accum_views, cache_view, sampler) =
        setup_transform_pass(&device, &queue, cw, ch);

    // Source: 4×4 white block at (10, 10). White RGB → luminance = 1.0 → mask value 255.
    let (source_data, origin, sw, sh) =
        make_source_rect(10, 10, 4, 4, [255, 255, 255, 255]);

    pass.set_floating_content(
        &device, &queue, &sampler,
        &accum_views, &cache_view,
        &source_data, origin, sw, sh, cw, ch,
        1, true, // target_is_mask = true
    );

    // Translate by (5, 5).
    let matrix = affine_translate(5.0, 5.0);

    let mut enc = encoder(&device);
    pass.commit_to_texture(&device, &mut enc, &queue, &target_tex, &target_view, mask_fmt, &matrix, origin, sw, sh, cw, ch);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, mask_fmt, cw, ch);

    // New position: (15, 15) to (18, 18) should have mask value near 255.
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let idx = ((15 + dy) * cw + 15 + dx) as usize;
            assert!(pixels[idx] > 200,
                "mask at ({},{}) should be ~255, got {}", 15 + dx, 15 + dy, pixels[idx]);
        }
    }

    // Original position and unrelated areas should be 0.
    assert_eq!(pixels[(10 * cw + 10) as usize], 0, "original pos should be 0");
    assert_eq!(pixels[0], 0, "corner should be 0");
}

// ============================================================================
// Affine math helpers (unit tests)
// ============================================================================

#[test]
fn affine_inverse_identity() {
    let inv = affine_inverse(&IDENTITY).unwrap();
    for i in 0..6 {
        assert!((inv[i] - IDENTITY[i]).abs() < 1e-6,
            "inverse of identity should be identity, element {} = {}", i, inv[i]);
    }
}

#[test]
fn affine_inverse_translate() {
    let m = affine_translate(10.0, -5.0);
    let inv = affine_inverse(&m).unwrap();
    // Inverse of translate(10, -5) = translate(-10, 5).
    assert!((inv[2] - (-10.0)).abs() < 1e-6, "inv tx = {}", inv[2]);
    assert!((inv[5] - 5.0).abs() < 1e-6, "inv ty = {}", inv[5]);
}

#[test]
fn affine_multiply_translate_chain() {
    let t1 = affine_translate(3.0, 4.0);
    let t2 = affine_translate(7.0, 6.0);
    let combined = affine_multiply(&t2, &t1);
    // translate(3,4) then translate(7,6) = translate(10, 10).
    assert!((combined[2] - 10.0).abs() < 1e-6);
    assert!((combined[5] - 10.0).abs() < 1e-6);
}
