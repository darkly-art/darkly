//! Transform and paste GPU integration tests: translation, rotation, paste, affine math.
//!
//! Tests TransformPass::commit_to_texture() with various transforms and undo.
//! Run with: `cargo test -p darkly --test transform`

use darkly::coord::CanvasRect;
use darkly::gpu::atlas::CanvasFrame;
use darkly::gpu::paint_target::{GpuPaintTarget, PaintPipelines};
use darkly::gpu::region_store::RegionStore;
use darkly::gpu::test_utils::*;
use darkly::gpu::transform::{
    affine_inverse, affine_multiply, affine_translate, Affine2D, TransformPass, IDENTITY,
};
use darkly::layer::LayerId;

/// Build a CanvasFrame for a test texture sized `(w, h)` at canvas origin (0, 0).
fn frame<'a>(tex: &'a wgpu::Texture, w: u32, h: u32) -> CanvasFrame<'a> {
    CanvasFrame {
        texture: tex,
        canvas_extent: CanvasRect::from_xywh(0, 0, w, h),
    }
}

fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test"),
    })
}

fn submit(queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) {
    queue.submit([encoder.finish()]);
}

fn pixel_at(pixels: &[u8], w: u32, x: u32, y: u32, bpp: u32) -> &[u8] {
    let offset = ((y * w + x) * bpp) as usize;
    &pixels[offset..offset + bpp as usize]
}

/// Build a `TransformPass` for use by these tests, which all exercise the
/// commit (live-target write) path. The derived-preview rework moved
/// preview rendering to the compositor's wrapper, so the tests no longer
/// need accumulator/cache views — but `TransformState` still owns a
/// per-target `preview_texture` for completeness, so the helpers below
/// supply a placeholder.
fn setup_transform_pass(
    device: &wgpu::Device,
    _queue: &wgpu::Queue,
    _canvas_w: u32,
    _canvas_h: u32,
) -> (TransformPass, wgpu::Sampler) {
    let pass = TransformPass::new(device);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (pass, sampler)
}

/// Allocate a placeholder preview texture matching the target's format and
/// dimensions. The commit-only tests don't read it back; it just satisfies
/// `TransformState`'s ownership invariant.
fn make_preview_placeholder(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-preview-placeholder"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

/// Run the commit pipeline against a target texture. Wraps the
/// `update_uniforms` → `render_commit` sequence into the single-call shape
/// the old tests expected.
#[allow(clippy::too_many_arguments)]
fn commit_to_texture(
    pass: &TransformPass,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    queue: &wgpu::Queue,
    target_tex: &wgpu::Texture,
    target_view: &wgpu::TextureView,
    matrix: &Affine2D,
    source_origin: (i32, i32),
    source_w: u32,
    source_h: u32,
    target_offset: (i32, i32),
    target_w: u32,
    target_h: u32,
    canvas_w: u32,
    canvas_h: u32,
) {
    pass.update_uniforms(
        queue,
        matrix,
        source_origin,
        source_w,
        source_h,
        target_offset,
        target_w,
        target_h,
        canvas_w,
        canvas_h,
    );
    pass.render_commit(device, encoder, target_tex, target_view);
}

/// Mirror of the production paste path: uploads RGBA pixel data and
/// allocates a placeholder preview texture sized to the canvas.
/// `target_format` matches the live target texture's format — RGBA8 for
/// regular layers, R8 when committing onto a mask (the commit shader's
/// `is_r8` branch maps the source's R channel into the single-channel
/// output).
#[allow(clippy::too_many_arguments)]
fn set_floating_content_rgba(
    pass: &mut TransformPass,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sampler: &wgpu::Sampler,
    rgba_data: &[u8],
    source_w: u32,
    source_h: u32,
    target_layer: LayerId,
    canvas_w: u32,
    canvas_h: u32,
    target_format: wgpu::TextureFormat,
) {
    let (preview_tex, preview_view) =
        make_preview_placeholder(device, target_format, canvas_w, canvas_h);
    pass.set_floating_content(
        device,
        queue,
        sampler,
        rgba_data,
        source_w,
        source_h,
        target_layer,
        target_format,
        preview_tex,
        preview_view,
        None,
    );
}

/// Helper: create flat RGBA pixel data with a solid-color rectangle.
/// Returns (rgba_data, origin, width, height).
fn make_source_rect(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    color: [u8; 4],
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

    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    // Source: 4×4 red block at (10, 10).
    let (source_data, origin, sw, sh) = make_source_rect(10, 10, 4, 4, [255, 0, 0, 255]);

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        wgpu::TextureFormat::Rgba8Unorm,
    );

    // Translate by (10, 10).
    let matrix = affine_translate(10.0, 10.0);

    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &matrix,
        origin,
        sw,
        sh,
        (0, 0),
        cw,
        ch,
        cw,
        ch,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // New position: (20, 20) to (23, 23) should be red.
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let p = pixel_at(&pixels, cw, 20 + dx, 20 + dy, 4);
            assert!(
                p[0] > 200,
                "pixel at ({},{}) should be red, got R={}",
                20 + dx,
                20 + dy,
                p[0]
            );
            assert!(
                p[3] > 200,
                "pixel at ({},{}) should be opaque, got A={}",
                20 + dx,
                20 + dy,
                p[3]
            );
        }
    }

    // Old position: (10, 10) to (13, 13) should be transparent (target was blank,
    // commit only writes to new position).
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let p = pixel_at(&pixels, cw, 10 + dx, 10 + dy, 4);
            assert_eq!(
                p[3],
                0,
                "old position ({},{}) should be transparent, A={}",
                10 + dx,
                10 + dy,
                p[3]
            );
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

    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    let (source_data, origin, sw, sh) = make_source_rect(5, 5, 4, 4, [0, 255, 0, 255]);

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        wgpu::TextureFormat::Rgba8Unorm,
    );

    // Save pre-commit state.
    let mut enc = encoder(&device);
    let snap = store.save_region(
        &mut enc,
        &frame(&target_tex, cw, ch),
        fmt,
        CanvasRect::from_xywh(0, 0, cw, ch),
    );
    submit(&queue, enc);

    // Commit with translation (15, 15).
    let matrix = affine_translate(15.0, 15.0);
    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &matrix,
        origin,
        sw,
        sh,
        (0, 0),
        cw,
        ch,
        cw,
        ch,
    );
    submit(&queue, enc);

    // Commit undo entry.
    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        LayerId::from_ffi(1),
        &frame(&target_tex, cw, ch),
        &snap,
        CanvasRect::from_xywh(0, 0, cw, ch),
    );
    submit(&queue, enc);

    // Verify green at new position.
    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    let p = pixel_at(&pixels, cw, 20, 20, 4);
    assert!(p[1] > 200, "should be green at new pos, G={}", p[1]);

    // Undo.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &frame(&target_tex, cw, ch));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    assert_eq!(
        pixel_at(&pixels, cw, 20, 20, 4)[3],
        0,
        "after undo, should be transparent"
    );
    assert_eq!(
        pixel_at(&pixels, cw, 5, 5, 4)[3],
        0,
        "after undo, original pos still transparent"
    );
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

    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    // Source: vertical line at x=2 in a 5×5 block.
    let (sw, sh) = (5u32, 5u32);
    let (ox, oy) = (10i32, 10i32);
    let mut source_data = vec![0u8; (sw * sh * 4) as usize];
    for py in 0..5u32 {
        let off = ((py * sw + 2) * 4) as usize;
        source_data[off..off + 4].copy_from_slice(&[0, 0, 255, 255]);
    }

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        wgpu::TextureFormat::Rgba8Unorm,
    );

    // Rotate 90° CW: matrix [0, 1, 0, -1, 0, 5]
    let matrix: Affine2D = [0.0, 1.0, 0.0, -1.0, 0.0, 5.0];

    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &matrix,
        (ox, oy),
        sw,
        sh,
        (0, 0),
        cw,
        ch,
        cw,
        ch,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // Horizontal line at y=12, x=10..14.
    for x in 10..15u32 {
        let p = pixel_at(&pixels, cw, x, 12, 4);
        assert!(
            p[2] > 200,
            "rotated line at ({},12) should be blue, B={}",
            x,
            p[2]
        );
        assert!(
            p[3] > 200,
            "rotated line at ({},12) should be opaque, A={}",
            x,
            p[3]
        );
    }

    // Original vertical line position (x=12, y=10..11) should be transparent.
    let p = pixel_at(&pixels, cw, 12, 10, 4);
    assert_eq!(
        p[3], 0,
        "original vert line pos (12,10) should be clear, A={}",
        p[3]
    );
    let p = pixel_at(&pixels, cw, 12, 11, 4);
    assert_eq!(
        p[3], 0,
        "original vert line pos (12,11) should be clear, A={}",
        p[3]
    );
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

    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    // Source: 8×8 magenta block at (20, 20).
    let (source_data, origin, sw, sh) = make_source_rect(20, 20, 8, 8, [255, 0, 255, 255]);

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        wgpu::TextureFormat::Rgba8Unorm,
    );

    // Commit with identity — pixels land at their original position.
    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &IDENTITY,
        origin,
        sw,
        sh,
        (0, 0),
        cw,
        ch,
        cw,
        ch,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // Center of pasted block.
    let p = pixel_at(&pixels, cw, 24, 24, 4);
    assert!(
        p[0] > 200 && p[2] > 200,
        "should be magenta, got [{},{},{},{}]",
        p[0],
        p[1],
        p[2],
        p[3]
    );
    assert!(p[3] > 200, "should be opaque");

    // Outside the block.
    assert_eq!(
        pixel_at(&pixels, cw, 0, 0, 4)[3],
        0,
        "outside should be transparent"
    );
    assert_eq!(
        pixel_at(&pixels, cw, 19, 20, 4)[3],
        0,
        "just outside should be transparent"
    );
    assert_eq!(
        pixel_at(&pixels, cw, 28, 20, 4)[3],
        0,
        "just outside right should be transparent"
    );
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

    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    let (source_data, origin, sw, sh) = make_source_rect(10, 10, 6, 6, [255, 255, 0, 255]);

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        wgpu::TextureFormat::Rgba8Unorm,
    );

    // Save pre-paste state.
    let mut enc = encoder(&device);
    let snap = store.save_region(
        &mut enc,
        &frame(&target_tex, cw, ch),
        fmt,
        CanvasRect::from_xywh(0, 0, cw, ch),
    );
    submit(&queue, enc);

    // Commit paste.
    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &IDENTITY,
        origin,
        sw,
        sh,
        (0, 0),
        cw,
        ch,
        cw,
        ch,
    );
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        LayerId::from_ffi(1),
        &frame(&target_tex, cw, ch),
        &snap,
        CanvasRect::from_xywh(0, 0, cw, ch),
    );
    submit(&queue, enc);

    // Verify yellow pixels.
    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    let p = pixel_at(&pixels, cw, 12, 12, 4);
    assert!(
        p[0] > 200 && p[1] > 200,
        "should be yellow, got [{},{},{},{}]",
        p[0],
        p[1],
        p[2],
        p[3]
    );

    // Undo.
    let mut enc = encoder(&device);
    let forward = store.restore_region(&mut enc, &entry, &frame(&target_tex, cw, ch));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);
    assert_eq!(
        pixel_at(&pixels, cw, 12, 12, 4)[3],
        0,
        "after undo, should be transparent"
    );

    // Redo.
    let mut enc = encoder(&device);
    let _backward = store.restore_region(&mut enc, &forward, &frame(&target_tex, cw, ch));
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

    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    // Source: semi-transparent red (alpha=128) at (10,10) size 4×4.
    let (source_data, origin, sw, sh) = make_source_rect(10, 10, 4, 4, [255, 0, 0, 128]);

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        wgpu::TextureFormat::Rgba8Unorm,
    );

    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &IDENTITY,
        origin,
        sw,
        sh,
        (0, 0),
        cw,
        ch,
        cw,
        ch,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, cw, ch);

    // At (12, 12): semi-transparent red over blue.
    let p = pixel_at(&pixels, cw, 12, 12, 4);
    assert!(
        p[0] > 100 && p[0] < 180,
        "blended R should be ~128, got {}",
        p[0]
    );
    assert!(
        p[2] > 80 && p[2] < 180,
        "blended B should be ~127, got {}",
        p[2]
    );
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
    let (target_tex, target_view) = create_test_texture_with_format(
        &device,
        &queue,
        cw,
        ch,
        &vec![0u8; (cw * ch) as usize],
        mask_fmt,
    );

    // TransformPass still uses Rgba8Unorm for its accumulator format (preview path).
    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    // Source: 4×4 white block at (10, 10). The R8 commit shader pulls the
    // R channel directly, so a white RGBA source maps to mask value 255.
    let (source_data, origin, sw, sh) = make_source_rect(10, 10, 4, 4, [255, 255, 255, 255]);

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        mask_fmt,
    );

    // Translate by (5, 5).
    let matrix = affine_translate(5.0, 5.0);

    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &matrix,
        origin,
        sw,
        sh,
        (0, 0),
        cw,
        ch,
        cw,
        ch,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, mask_fmt, cw, ch);

    // New position: (15, 15) to (18, 18) should have mask value near 255.
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let idx = ((15 + dy) * cw + 15 + dx) as usize;
            assert!(
                pixels[idx] > 200,
                "mask at ({},{}) should be ~255, got {}",
                15 + dx,
                15 + dy,
                pixels[idx]
            );
        }
    }

    // Original position and unrelated areas should be 0.
    assert_eq!(
        pixels[(10 * cw + 10) as usize],
        0,
        "original pos should be 0"
    );
    assert_eq!(pixels[0], 0, "corner should be 0");
}

// ============================================================================
// Affine math helpers (unit tests)
// ============================================================================

#[test]
fn affine_inverse_identity() {
    let inv = affine_inverse(&IDENTITY).unwrap();
    for i in 0..6 {
        assert!(
            (inv[i] - IDENTITY[i]).abs() < 1e-6,
            "inverse of identity should be identity, element {} = {}",
            i,
            inv[i]
        );
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

/// Transform-commit onto a paste-extent layer at non-zero canvas offset:
/// the source must land at the requested canvas position, not at
/// canvas-pos minus offset. Regression guard for transform_commit.wgsl
/// `target_offset` migration.
#[test]
fn transform_commit_onto_offset_layer_lands_at_canvas_coords() {
    let (device, queue) = test_device();
    let (cw, ch) = (256u32, 256u32);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Layer texture is 200×200 placed at canvas (-50, -50) — paste-extent
    // configuration. Layer-local (0..200, 0..200) maps to canvas (-50..150).
    let target_off = (-50i32, -50i32);
    let (target_w, target_h) = (200u32, 200u32);
    let (target_tex, target_view) = create_test_texture(
        &device,
        &queue,
        target_w,
        target_h,
        &vec![0u8; (target_w * target_h * 4) as usize],
    );

    let (mut pass, sampler) = setup_transform_pass(&device, &queue, cw, ch);

    // Source: 4×4 green block at canvas (10, 10). Identity transform — the
    // block should appear unchanged at canvas (10, 10), which is layer-local
    // (60, 60) on the offset target.
    let (source_data, origin, sw, sh) = make_source_rect(10, 10, 4, 4, [0, 255, 0, 255]);

    set_floating_content_rgba(
        &mut pass,
        &device,
        &queue,
        &sampler,
        &source_data,
        sw,
        sh,
        LayerId::from_ffi(1),
        cw,
        ch,
        wgpu::TextureFormat::Rgba8Unorm,
    );

    let mut enc = encoder(&device);
    commit_to_texture(
        &pass,
        &device,
        &mut enc,
        &queue,
        &target_tex,
        &target_view,
        &IDENTITY,
        origin,
        sw,
        sh,
        target_off,
        target_w,
        target_h,
        cw,
        ch,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &target_tex, fmt, target_w, target_h);

    // Canvas (10, 10) → layer-local (60, 60). Block is 4×4 → covers (60..64, 60..64).
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let p = pixel_at(&pixels, target_w, 60 + dx, 60 + dy, 4);
            assert!(
                p[1] > 200,
                "layer-local ({},{}) should be green, got G={}",
                60 + dx,
                60 + dy,
                p[1]
            );
            assert_eq!(
                p[3],
                255,
                "layer-local ({},{}) alpha should be 255",
                60 + dx,
                60 + dy
            );
        }
    }

    // The OLD buggy mapping would have placed the block at layer-local
    // (10, 10) — that position must be untouched.
    for dy in 0..4u32 {
        for dx in 0..4u32 {
            let p = pixel_at(&pixels, target_w, 10 + dx, 10 + dy, 4);
            assert_eq!(
                p[3],
                0,
                "layer-local ({},{}) should still be transparent (would be wrong-place commit)",
                10 + dx,
                10 + dy
            );
        }
    }
}

/// Regression for the canvas-coord storage refactor (see plan
/// `mossy-sleeping-flame.md`): a floating transform's `cancel_snapshot`
/// must restore at the correct canvas position even if the layer's
/// canvas extent changes between setup and cancel. This simulates that
/// scenario directly with `RegionStore`: save into a 256×256 layer at
/// canvas (0, 0), simulate a negative-direction grow that shifts the
/// layer's local-coord origin to (256, 256) in a 512×512 texture, then
/// `restore_from_scratch` using the canvas-coord snapshot. Pre-fix
/// (`Snapshot.saved: LayerRect`) the saved rect would still name the
/// pre-grow layer-local frame and write to the wrong texels in the new
/// frame; canvas-coord storage round-trips cleanly via
/// `canvas_to_layer_rect`.
#[test]
fn cancel_floating_after_layer_grow() {
    use wgpu::TextureUsages;
    let (device, queue) = test_device();
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let (init_w, init_h) = (256u32, 256u32);

    // Initial 256×256 layer filled red — represents the pre-floating layer
    // pixels at canvas (0, 0)..(256, 256).
    let red: Vec<u8> = (0..init_w * init_h)
        .flat_map(|_| [255u8, 0, 0, 255])
        .collect();
    let (initial_tex, _) = create_test_texture(&device, &queue, init_w, init_h, &red);
    let initial_frame = CanvasFrame {
        texture: &initial_tex,
        canvas_extent: CanvasRect::from_xywh(0, 0, init_w, init_h),
    };

    let mut store = RegionStore::with_capacity(&device, init_w, init_h, 4 * 1024 * 1024);

    // Floating transform setup snapshots a 100×100 region at canvas (50, 50).
    let saved_canvas_rect = CanvasRect::from_xywh(50, 50, 100, 100);
    let mut enc = encoder(&device);
    let mut cancel_snapshot = store.save_region(&mut enc, &initial_frame, fmt, saved_canvas_rect);
    submit(&queue, enc);

    // Simulate a grow that shifts the layer's local frame: new 512×512
    // texture at canvas (-256, -256), old contents land at layer-local
    // (256, 256).
    let (new_w, new_h) = (512u32, 512u32);
    let new_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grown-layer"),
        size: wgpu::Extent3d {
            width: new_w,
            height: new_h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_SRC
            | TextureUsages::COPY_DST
            | TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let mut enc = encoder(&device);
    enc.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &initial_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &new_tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 256,
                y: 256,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: init_w,
            height: init_h,
            depth_or_array_layers: 1,
        },
    );
    store.grow_scratch_preserving(&device, &mut enc, new_w, new_h, 256, 256);
    submit(&queue, enc);

    // After grow, the engine widens snap.saved to the full new canvas
    // extent so commits/restores that touch newly-grown areas are still
    // contained. The original saved canvas region is untouched.
    cancel_snapshot.saved = CanvasRect::from_xywh(-256, -256, new_w, new_h);

    let new_frame = CanvasFrame {
        texture: &new_tex,
        canvas_extent: CanvasRect::from_xywh(-256, -256, new_w, new_h),
    };

    // Stomp the layer at the saved canvas region with green to prove that
    // restore_from_scratch puts the saved red pixels back at the right
    // canvas position (not the wrong layer-local origin).
    let pipelines = PaintPipelines::new(&device, &queue);
    let new_view = new_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let target = GpuPaintTarget {
        texture: &new_tex,
        view: &new_view,
        format: fmt,
        width: new_w,
        height: new_h,
        offset_x: -256,
        offset_y: -256,
        canvas_width: new_w,
        canvas_height: new_h,
    };
    let mut enc = encoder(&device);
    target.fill_rect(
        &mut enc,
        &pipelines,
        &queue,
        [50, 50, 100, 100],
        [0, 255, 0, 255],
    );
    submit(&queue, enc);

    // Cancel: restore_from_scratch using the canvas-coord saved rect.
    let mut enc = encoder(&device);
    store.restore_from_scratch(&mut enc, &cancel_snapshot, &new_frame, saved_canvas_rect);
    submit(&queue, enc);

    // The saved canvas region (50..150, 50..150) must be red again.
    // canvas (50, 50) → layer-local (306, 306) in the new frame.
    let pixels = readback_texture(&device, &queue, &new_tex, fmt, new_w, new_h);
    let p = pixel_at(&pixels, new_w, 306, 306, 4);
    assert_eq!(
        p,
        &[255, 0, 0, 255],
        "after cancel, canvas (50, 50) (= layer-local (306, 306)) must be \
         restored to the pre-floating red, got {:?}",
        p,
    );
    let q = pixel_at(&pixels, new_w, 405, 405, 4); // canvas (149, 149)
    assert_eq!(
        q,
        &[255, 0, 0, 255],
        "near far edge of saved canvas region should also be red, got {:?}",
        q,
    );

    // The OLD buggy mapping would have placed the restore at layer-local
    // (50, 50) — that position must remain untouched (zero in the
    // newly-grown area of the layer).
    let buggy = pixel_at(&pixels, new_w, 50, 50, 4);
    assert_eq!(
        buggy[3], 0,
        "layer-local (50, 50) (canvas (-206, -206)) is in the grown area \
         and must be transparent — non-zero alpha here would mean the \
         restore landed at the stale pre-grow layer origin, A={}",
        buggy[3],
    );
}
