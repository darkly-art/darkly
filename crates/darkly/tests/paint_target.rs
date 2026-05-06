//! GpuPaintTarget and readback GPU integration tests.
//!
//! Tests compositing, alpha blending, erasing, masking, and GPU readback.
//! Run with: `cargo test -p darkly --test paint_target`

use darkly::gpu::paint_target::{GpuPaintTarget, PaintPipelines};
use darkly::gpu::readback;
use darkly::gpu::test_utils::*;

fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test"),
    })
}

fn submit(queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) {
    queue.submit([encoder.finish()]);
}

// ============================================================================
// GpuPaintTarget tests
// ============================================================================

/// composite_circle: paint red circle, verify center is red and corners are transparent.
#[test]
fn paint_target_composite_circle() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        64.0,
        64.0,
        10.0,
        [255, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Center pixel (64, 64) should be red.
    let c = ((64 * w + 64) * 4) as usize;
    assert_eq!(pixels[c], 255, "center R={}, expected 255", pixels[c]);
    assert_eq!(pixels[c + 1], 0, "center G={}, expected 0", pixels[c + 1]);
    assert_eq!(pixels[c + 2], 0, "center B={}, expected 0", pixels[c + 2]);
    assert_eq!(
        pixels[c + 3],
        255,
        "center A={}, expected 255",
        pixels[c + 3]
    );

    // Corner (0, 0) should be transparent.
    assert_eq!(pixels[3], 0, "corner alpha={}, expected 0", pixels[3]);

    // Pixel inside circle (64, 57) — 7px from center, radius 10.
    let inside = ((57 * w + 64) * 4) as usize;
    assert!(
        pixels[inside + 3] > 0,
        "inside circle should be non-transparent"
    );

    // Pixel outside circle (64, 50) — 14px from center, radius 10.
    let outside = ((50 * w + 64) * 4) as usize;
    assert_eq!(
        pixels[outside + 3],
        0,
        "outside circle should be transparent, got A={}",
        pixels[outside + 3]
    );
}

/// Alpha blending: composite circle on semi-transparent background.
#[test]
fn paint_target_alpha_blending() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let bg: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 128]).collect();
    let (tex, view) = create_test_texture(&device, &queue, w, h, &bg);
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    // Paint red circle at center with 50% alpha.
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        64.0,
        64.0,
        10.0,
        [255, 0, 0, 128],
        1.0,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let c = ((64 * w + 64) * 4) as usize;
    let (r, g, b, a) = (pixels[c], pixels[c + 1], pixels[c + 2], pixels[c + 3]);

    // Source-over: out.a = src.a + dst.a * (1 - src.a) > 128
    assert!(r > 0, "blended pixel should have red, got R={r}");
    assert!(b > 0, "blended pixel should have blue, got B={b}");
    assert!(a > 128, "blended alpha should be > 128, got A={a}");
    assert_eq!(g, 0, "green should remain 0, got G={g}");
}

/// erase_circle: fill red, erase center, verify center alpha is 0.
#[test]
fn paint_target_erase_circle() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, view) = create_test_texture(&device, &queue, w, h, &red);
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    let mut enc = encoder(&device);
    target.erase_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let c = ((64 * w + 64) * 4) as usize;

    // Center alpha should be 0 (erased).
    assert_eq!(
        pixels[c + 3],
        0,
        "center alpha should be 0, got {}",
        pixels[c + 3]
    );
    // RGB should be preserved.
    assert_eq!(pixels[c], 255, "center R should be 255, got {}", pixels[c]);
    // Corner should be unchanged.
    assert_eq!(pixels[3], 255, "corner alpha should be 255");
}

/// R8 mask target: paint black on fully-revealed mask.
#[test]
fn paint_target_r8_mask() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::R8Unorm;

    let white: Vec<u8> = vec![255u8; (w * h) as usize];
    let (tex, view) = create_test_texture_with_format(&device, &queue, w, h, &white, fmt);
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    // Composite black → luminance 0 → mask toward 0.
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        64.0,
        64.0,
        10.0,
        [0, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    let center = (64 * w + 64) as usize;
    assert!(
        pixels[center] < 10,
        "center mask should be near 0, got {}",
        pixels[center]
    );
    assert_eq!(
        pixels[0], 255,
        "corner mask should be 255, got {}",
        pixels[0]
    );
}

/// Selection masking: left half selected, right half not.
#[test]
fn paint_target_selection_masking() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);

    // Selection mask: left half = 255, right half = 0.
    let mut sel_data = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w / 2 {
            sel_data[(y * w + x) as usize] = 255;
        }
    }
    let (_sel_tex, sel_view) = create_test_texture_with_format(
        &device,
        &queue,
        w,
        h,
        &sel_data,
        wgpu::TextureFormat::R8Unorm,
    );

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("test-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let sel_bind_group = pipelines.create_selection_bind_group(&device, &sel_view, &sampler);

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    let mut enc = encoder(&device);
    target.composite_circle_with_selection(
        &mut enc,
        &pipelines,
        &queue,
        64.0,
        64.0,
        30.0,
        [255, 0, 0, 255],
        1.0,
        &sel_bind_group,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Left side, within circle (48, 64) — 16px from center, well within r=30.
    let left = ((64 * w + 48) * 4) as usize;
    assert!(
        pixels[left + 3] > 0,
        "left (selected) should have paint, A={}",
        pixels[left + 3]
    );

    // Right side, within circle (80, 64) — 16px from center, within r=30 but unselected.
    let right = ((64 * w + 80) * 4) as usize;
    assert_eq!(
        pixels[right + 3],
        0,
        "right (unselected) should be transparent, A={}",
        pixels[right + 3]
    );
}

// ============================================================================
// Readback tests
// ============================================================================

/// Readback round-trip: known pattern → readback → exact match.
#[test]
fn readback_round_trip() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            data[i] = y as u8;
            data[i + 1] = x as u8;
            data[i + 2] = 42;
            data[i + 3] = 255;
        }
    }
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &data);

    let readback = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(readback, data, "readback should exactly match input");
}

/// Readback sub-rect: read top-left 64×64 of a 128×128 texture.
#[test]
fn readback_sub_rect() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Quadrant colors.
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (r, g, b) = match (x < 64, y < 64) {
                (true, true) => (255, 0, 0),
                (false, true) => (0, 255, 0),
                (true, false) => (0, 0, 255),
                (false, false) => (255, 255, 0),
            };
            data[i] = r;
            data[i + 1] = g;
            data[i + 2] = b;
            data[i + 3] = 255;
        }
    }
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &data);

    // Read only top-left 64×64.
    let mut enc = encoder(&device);
    let request = readback::request_readback(&device, &mut enc, &tex, fmt, [0, 0, 64, 64]);
    submit(&queue, enc);
    let pixels = request.blocking_read(&device);

    assert_eq!(pixels.len(), (64 * 64 * 4) as usize);

    // All pixels should be red.
    for y in 0..64u32 {
        for x in 0..64u32 {
            let i = ((y * 64 + x) * 4) as usize;
            assert_eq!(
                &pixels[i..i + 4],
                &[255, 0, 0, 255],
                "pixel ({x},{y}) should be red, got {:?}",
                &pixels[i..i + 4]
            );
        }
    }
}

/// Painting on an offset paste-extent layer: the painted pixels must land at
/// the canvas position requested, not at (canvas_pos − offset).
#[test]
fn paint_target_composite_circle_on_offset_layer() {
    let (device, queue) = test_device();
    let canvas_w: u32 = 256;
    let canvas_h: u32 = 256;
    // Layer's (0,0) sits at canvas (-100, -100). Painting at canvas (50, 50)
    // should land at layer-local (150, 150).
    let layer_off_x: i32 = -100;
    let layer_off_y: i32 = -100;
    let (lw, lh) = (400u32, 400u32);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) =
        create_test_texture(&device, &queue, lw, lh, &vec![0u8; (lw * lh * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: lw,
        height: lh,
        offset_x: layer_off_x,
        offset_y: layer_off_y,
        canvas_width: canvas_w,
        canvas_height: canvas_h,
    };

    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        50.0,
        50.0,
        10.0,
        [0, 255, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, lw, lh);

    // Canvas (50, 50) → layer-local (50 - (-100), 50 - (-100)) = (150, 150).
    let cx_local = (50 - layer_off_x) as u32;
    let cy_local = (50 - layer_off_y) as u32;
    let c = ((cy_local * lw + cx_local) * 4) as usize;
    assert_eq!(pixels[c], 0);
    assert_eq!(
        pixels[c + 1],
        255,
        "expected green at layer-local (150,150)"
    );
    assert_eq!(pixels[c + 2], 0);
    assert_eq!(pixels[c + 3], 255);

    // The OLD buggy mapping would have painted at canvas (50,50) interpreted
    // as layer-local — i.e. at layer-local (50, 50). That position must be
    // empty.
    let bug = ((50u32 * lw + 50) * 4) as usize;
    assert_eq!(
        pixels[bug + 3],
        0,
        "layer-local (50,50) should be untouched (would be wrong-place paint)"
    );
}

/// fill_rect on an offset paste-extent layer: rect input is canvas-space.
/// A canvas-space rect at (canvas_x, canvas_y) lands at layer-local
/// (canvas_x − offset_x, canvas_y − offset_y). Regression for the
/// `clear_rect`/`fill_rect` canvas-space API contract (P1c).
#[test]
fn paint_target_fill_rect_canvas_space_on_offset_layer() {
    let (device, queue) = test_device();
    let canvas_w: u32 = 256;
    let canvas_h: u32 = 256;
    let (lw, lh) = (300u32, 300u32);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let off_x: i32 = -50;
    let off_y: i32 = -50;
    let (tex, view) =
        create_test_texture(&device, &queue, lw, lh, &vec![0u8; (lw * lh * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: lw,
        height: lh,
        offset_x: off_x,
        offset_y: off_y,
        canvas_width: canvas_w,
        canvas_height: canvas_h,
    };

    // Canvas-space rect at (10, 10) size (20, 20). Maps to layer-local
    // (10 - (-50), 10 - (-50)) = (60, 60).
    let mut enc = encoder(&device);
    target.fill_rect(
        &mut enc,
        &pipelines,
        &queue,
        [10, 10, 20, 20],
        [0, 0, 255, 255],
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, lw, lh);
    // Layer-local (65, 65) — interior of the rect.
    let lx = (15 - off_x) as u32;
    let ly = (15 - off_y) as u32;
    let c = ((ly * lw + lx) * 4) as usize;
    assert_eq!(
        pixels[c + 2],
        255,
        "rect interior should be blue at layer-local ({lx},{ly})"
    );
    assert_eq!(pixels[c + 3], 255);
    // Layer-local (50, 50) — would be the OLD (target-local) interpretation;
    // must be transparent under the canvas-space contract.
    let outside = ((50u32 * lw + 50) * 4) as usize;
    assert_eq!(
        pixels[outside + 3],
        0,
        "layer-local (50,50) should be untouched (canvas-space input lands at layer-local (60,60))"
    );
}

/// Canvas-negative origin: a fill_rect at canvas (-30, 10) on a layer with
/// offset (-50, -50) should land at layer-local (20, 60). Confirms the
/// `[i32; 4]` rect contract handles negative canvas coordinates.
#[test]
fn paint_target_fill_rect_canvas_negative_origin_on_offset_layer() {
    let (device, queue) = test_device();
    let canvas_w: u32 = 128;
    let canvas_h: u32 = 128;
    let (lw, lh) = (200u32, 200u32);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let off_x: i32 = -50;
    let off_y: i32 = -50;
    let (tex, view) =
        create_test_texture(&device, &queue, lw, lh, &vec![0u8; (lw * lh * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: lw,
        height: lh,
        offset_x: off_x,
        offset_y: off_y,
        canvas_width: canvas_w,
        canvas_height: canvas_h,
    };

    // Canvas-space rect at (-30, 10) size (10, 10). Maps to layer-local (20, 60).
    let mut enc = encoder(&device);
    target.fill_rect(
        &mut enc,
        &pipelines,
        &queue,
        [-30, 10, 10, 10],
        [0, 255, 0, 255],
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, lw, lh);
    // Layer-local (25, 65) — interior.
    let lx = (-25 - off_x) as u32;
    let ly = (15 - off_y) as u32;
    let c = ((ly * lw + lx) * 4) as usize;
    assert_eq!(
        pixels[c + 1],
        255,
        "rect interior should be green at layer-local ({lx},{ly})"
    );
}
