//! RegionScratch GPU integration tests: save/restore, partial rects, undo/redo.
//!
//! Run with: `cargo test -p darkly --test region_store`

use darkly::coord::CanvasRect;
use darkly::gpu::atlas::CanvasFrame;
use darkly::gpu::region_store::RegionScratch;
use darkly::gpu::test_utils::*;

fn cr(x: i32, y: i32, w: u32, h: u32) -> CanvasRect {
    CanvasRect::from_xywh(x, y, w, h)
}

/// Build a CanvasFrame for a test texture sized `(w, h)` at canvas origin (0, 0).
/// Tests treat the bare `wgpu::Texture` as if it were a layer-aligned canvas
/// texture starting at the origin.
fn frame<'a>(tex: &'a wgpu::Texture, w: u32, h: u32) -> CanvasFrame<'a> {
    CanvasFrame {
        texture: tex,
        canvas_extent: cr(0, 0, w, h),
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

fn write_texture(queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32, bpp: u32, data: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * bpp),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}

/// Save/restore round-trip: fill red → save → overwrite blue → restore → assert red.
#[test]
fn region_store_save_restore_round_trip() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &red);
    let frame = frame(&tex, w, h);

    let mut store = RegionScratch::new(&device, w, h);

    // Save region.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame, fmt, cr(0, 0, w, h));
    let (entry, _req) = store.commit_region(
        &mut enc,
        &device,
        darkly::layer::LayerId::from_ffi(1),
        &frame,
        &snap,
        cr(0, 0, w, h),
    );
    submit(&queue, enc);

    // Overwrite with blue.
    let blue: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 255]).collect();
    write_texture(&queue, &tex, w, h, 4, &blue);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        &pixels[0..4],
        &[0, 0, 255, 255],
        "should be blue before restore"
    );

    // Restore.
    let mut enc = encoder(&device);
    let (_forward, _req) = store.restore_region(&mut enc, &device, &entry, &frame);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        &pixels[0..4],
        &[255, 0, 0, 255],
        "should be red after restore"
    );
}

/// Partial rect: save inner 64×64, overwrite full texture, restore only inner rect.
#[test]
fn region_store_partial_rect() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &red);
    let frame = frame(&tex, w, h);

    let mut store = RegionScratch::new(&device, w, h);

    // Save only inner 64×64 rect at (32, 32).
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame, fmt, cr(32, 32, 64, 64));
    let (entry, _req) = store.commit_region(
        &mut enc,
        &device,
        darkly::layer::LayerId::from_ffi(1),
        &frame,
        &snap,
        cr(32, 32, 64, 64),
    );
    submit(&queue, enc);

    // Overwrite entire texture with blue.
    let blue: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 255]).collect();
    write_texture(&queue, &tex, w, h, 4, &blue);

    // Restore only the inner rect.
    let mut enc = encoder(&device);
    let (_forward, _req) = store.restore_region(&mut enc, &device, &entry, &frame);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Inner rect should be red.
    let inner_offset = ((32 * w + 32) * 4) as usize;
    assert_eq!(
        &pixels[inner_offset..inner_offset + 4],
        &[255, 0, 0, 255],
        "inner should be red"
    );

    // Outer border should still be blue.
    assert_eq!(
        &pixels[0..4],
        &[0, 0, 255, 255],
        "outer (0,0) should be blue"
    );
    let bottom_right = (((h - 1) * w + (w - 1)) * 4) as usize;
    assert_eq!(
        &pixels[bottom_right..bottom_right + 4],
        &[0, 0, 255, 255],
        "outer bottom-right should be blue"
    );
}

/// Redo round-trip: save red → overwrite blue → commit → undo → redo.
#[test]
fn region_store_redo_round_trip() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &red);
    let frame = frame(&tex, w, h);

    let mut store = RegionScratch::new(&device, w, h);

    // Save red state.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame, fmt, cr(0, 0, w, h));
    let (entry_a, _req) = store.commit_region(
        &mut enc,
        &device,
        darkly::layer::LayerId::from_ffi(1),
        &frame,
        &snap,
        cr(0, 0, w, h),
    );
    submit(&queue, enc);

    // Overwrite with blue.
    let blue: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 255]).collect();
    write_texture(&queue, &tex, w, h, 4, &blue);

    // Undo: restore red, get forward entry (blue).
    let mut enc = encoder(&device);
    let (entry_b, _req) = store.restore_region(&mut enc, &device, &entry_a, &frame);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(&pixels[0..4], &[255, 0, 0, 255], "undo should restore red");

    // Redo: restore blue.
    let mut enc = encoder(&device);
    let (_entry_c, _req) = store.restore_region(&mut enc, &device, &entry_b, &frame);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(&pixels[0..4], &[0, 0, 255, 255], "redo should restore blue");
}

/// R8 format: save/restore round-trip on R8Unorm texture.
#[test]
fn region_store_r8_format() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::R8Unorm;

    let white: Vec<u8> = vec![255u8; (w * h) as usize];
    let (tex, _view) = create_test_texture_with_format(&device, &queue, w, h, &white, fmt);
    let frame = frame(&tex, w, h);

    let mut store = RegionScratch::new(&device, w, h);

    // Save.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame, fmt, cr(0, 0, w, h));
    let (entry, _req) = store.commit_region(
        &mut enc,
        &device,
        darkly::layer::LayerId::from_ffi(1),
        &frame,
        &snap,
        cr(0, 0, w, h),
    );
    submit(&queue, enc);

    // Overwrite with 0.
    let black: Vec<u8> = vec![0u8; (w * h) as usize];
    write_texture(&queue, &tex, w, h, 1, &black);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixels[0], 0, "should be 0 before restore");

    // Restore.
    let mut enc = encoder(&device);
    let (_forward, _req) = store.restore_region(&mut enc, &device, &entry, &frame);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixels[0], 255, "should be 255 after restore");
}

/// Regression: brush stroke undo flow saves the FULL layer at stroke start,
/// then commits only the diff sub-rect at stroke end. On undo, the buffer
/// must hold the pre-stroke pixels at the sub-rect's *layer-space* location
/// — not whatever pixels happened to live at scratch's top-left.
///
/// Was broken when scratch was switched to "always indexed at (0,0)":
/// commit_region read scratch[0..w, 0..h] regardless of the rect's xy, so
/// undo blitted the layer's top-left pixels onto the changed region.
#[test]
fn region_store_save_full_commit_subrect() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Top-left 32×32 = red, center 32×32 at (48,48) = green, rest = blue.
    // Distinct colors per region so we can tell if undo restored the wrong slice.
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            let (r, g, b) = if x < 32 && y < 32 {
                (255u8, 0, 0)
            } else if (48..80).contains(&x) && (48..80).contains(&y) {
                (0, 255, 0)
            } else {
                (0, 0, 255)
            };
            data[idx] = r;
            data[idx + 1] = g;
            data[idx + 2] = b;
            data[idx + 3] = 255;
        }
    }
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &data);
    let frame = frame(&tex, w, h);

    let mut store = RegionScratch::new(&device, w, h);

    // Stroke begin — save the full layer.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame, fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    // Simulate dabs landing on the green center, turning it white.
    let mut painted = data.clone();
    for y in 48..80 {
        for x in 48..80 {
            let idx = ((y * w + x) * 4) as usize;
            painted[idx] = 255;
            painted[idx + 1] = 255;
            painted[idx + 2] = 255;
        }
    }
    write_texture(&queue, &tex, w, h, 4, &painted);

    // Stroke end — diff_rect would return the painted center; commit that sub-rect.
    let mut enc = encoder(&device);
    let (entry, _req) = store.commit_region(
        &mut enc,
        &device,
        darkly::layer::LayerId::from_ffi(1),
        &frame,
        &snap,
        cr(48, 48, 32, 32),
    );
    submit(&queue, enc);

    // Undo.
    let mut enc = encoder(&device);
    let (_forward, _req) = store.restore_region(&mut enc, &device, &entry, &frame);
    submit(&queue, enc);

    // Center must be green (pre-stroke state at that location), not red
    // (the top-left color that the bug would copy).
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center_idx = ((50 * w + 50) * 4) as usize;
    assert_eq!(
        &pixels[center_idx..center_idx + 4],
        &[0, 255, 0, 255],
        "center pixel after undo must be the pre-stroke green, got {:?}",
        &pixels[center_idx..center_idx + 4]
    );
}

/// Lock in the new debug-mode contract: `commit_region` must reject a rect
/// that escapes the saved snapshot. Caller bug, not RegionScratch bug — but
/// the assert turns "silent corruption from reading uninitialised scratch"
/// into "loud panic during dev/test."
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "not contained")]
fn region_store_commit_outside_saved_panics() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let blank = vec![0u8; (w * h * 4) as usize];
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &blank);
    let frame = frame(&tex, w, h);

    let mut store = RegionScratch::new(&device, w, h);

    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame, fmt, cr(0, 0, 32, 32));
    submit(&queue, enc);

    // Commit at a rect that is NOT contained in the saved (0,0,32,32) area.
    let mut enc = encoder(&device);
    let _ = store.commit_region(
        &mut enc,
        &device,
        darkly::layer::LayerId::from_ffi(1),
        &frame,
        &snap,
        cr(100, 100, 32, 32),
    );
}
