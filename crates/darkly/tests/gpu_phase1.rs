//! Phase 1 GPU integration tests.
//!
//! These tests require a GPU adapter (hardware or software fallback).
//! Run with: `cargo test -p darkly --test gpu_phase1`

use darkly::gpu::test_utils::*;
use darkly::gpu::readback;
use darkly::gpu::region_store::RegionStore;
use darkly::gpu::paint_target::{GpuPaintTarget, PaintPipelines};

fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") })
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
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

// ============================================================================
// RegionStore tests
// ============================================================================

/// Save/restore round-trip: fill red → save → overwrite blue → restore → assert red.
#[test]
fn region_store_save_restore_round_trip() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &red);

    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // Save region.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Overwrite with blue.
    let blue: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 255]).collect();
    write_texture(&queue, &tex, w, h, 4, &blue);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(&pixels[0..4], &[0, 0, 255, 255], "should be blue before restore");

    // Restore.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(&pixels[0..4], &[255, 0, 0, 255], "should be red after restore");
}

/// Partial rect: save inner 64×64, overwrite full texture, restore only inner rect.
#[test]
fn region_store_partial_rect() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &red);

    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // Save only inner 64×64 rect at (32, 32).
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [32, 32, 64, 64]);
    let entry = store.commit_region(&mut enc, 1, fmt, [32, 32, 64, 64]);
    submit(&queue, enc);

    // Overwrite entire texture with blue.
    let blue: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 255]).collect();
    write_texture(&queue, &tex, w, h, 4, &blue);

    // Restore only the inner rect.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Inner rect should be red.
    let inner_offset = ((32 * w + 32) * 4) as usize;
    assert_eq!(&pixels[inner_offset..inner_offset + 4], &[255, 0, 0, 255], "inner should be red");

    // Outer border should still be blue.
    assert_eq!(&pixels[0..4], &[0, 0, 255, 255], "outer (0,0) should be blue");
    let bottom_right = (((h - 1) * w + (w - 1)) * 4) as usize;
    assert_eq!(&pixels[bottom_right..bottom_right + 4], &[0, 0, 255, 255], "outer bottom-right should be blue");
}

/// Redo round-trip: save red → overwrite blue → commit → undo → redo.
#[test]
fn region_store_redo_round_trip() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, _view) = create_test_texture(&device, &queue, w, h, &red);

    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // Save red state.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    let entry_a = store.commit_region(&mut enc, 1, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Overwrite with blue.
    let blue: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 255]).collect();
    write_texture(&queue, &tex, w, h, 4, &blue);

    // Undo: restore red, get forward entry (blue).
    let mut enc = encoder(&device);
    let entry_b = store.restore_region(&mut enc, &entry_a, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(&pixels[0..4], &[255, 0, 0, 255], "undo should restore red");

    // Redo: restore blue.
    let mut enc = encoder(&device);
    let _entry_c = store.restore_region(&mut enc, &entry_b, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(&pixels[0..4], &[0, 0, 255, 255], "redo should restore blue");
}

/// Ring buffer eviction: push entries until oldest are evicted, newest still works.
#[test]
fn region_store_ring_buffer_eviction() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Padded row for 64px RGBA = 256 bytes. 256 * 64 = 16384 bytes per entry.
    // Capacity for ~3 entries.
    let entry_size = 256u64 * 64;
    let capacity = entry_size * 3;

    let (tex, _view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let mut store = RegionStore::with_capacity(&device, w, h, capacity);

    // Push 4 entries — the first should be evicted.
    let mut entries = Vec::new();
    for i in 0..4u8 {
        let color: Vec<u8> = (0..w * h).flat_map(|_| [i * 60, 0, 0, 255]).collect();
        write_texture(&queue, &tex, w, h, 4, &color);

        let mut enc = encoder(&device);
        store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
        let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, w, h]);
        submit(&queue, enc);
        entries.push(entry);
    }

    // Overwrite with green, then restore newest entry.
    let green: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 255, 0, 255]).collect();
    write_texture(&queue, &tex, w, h, 4, &green);

    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entries[3], &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    // Entry 3 saved color [180, 0, 0, 255].
    assert_eq!(pixels[0], 180, "newest entry should be restorable: expected 180, got {}", pixels[0]);
}

/// R8 format: save/restore round-trip on R8Unorm texture.
#[test]
fn region_store_r8_format() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::R8Unorm;

    let white: Vec<u8> = vec![255u8; (w * h) as usize];
    let (tex, _view) = create_test_texture_with_format(&device, &queue, w, h, &white, fmt);

    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // Save.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Overwrite with 0.
    let black: Vec<u8> = vec![0u8; (w * h) as usize];
    write_texture(&queue, &tex, w, h, 1, &black);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixels[0], 0, "should be 0 before restore");

    // Restore.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixels[0], 255, "should be 255 after restore");
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
    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };

    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0, [255, 0, 0, 255], 1.0);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Center pixel (64, 64) should be red.
    let c = ((64 * w + 64) * 4) as usize;
    assert_eq!(pixels[c], 255, "center R={}, expected 255", pixels[c]);
    assert_eq!(pixels[c + 1], 0, "center G={}, expected 0", pixels[c + 1]);
    assert_eq!(pixels[c + 2], 0, "center B={}, expected 0", pixels[c + 2]);
    assert_eq!(pixels[c + 3], 255, "center A={}, expected 255", pixels[c + 3]);

    // Corner (0, 0) should be transparent.
    assert_eq!(pixels[3], 0, "corner alpha={}, expected 0", pixels[3]);

    // Pixel inside circle (64, 57) — 7px from center, radius 10.
    let inside = ((57 * w + 64) * 4) as usize;
    assert!(pixels[inside + 3] > 0, "inside circle should be non-transparent");

    // Pixel outside circle (64, 50) — 14px from center, radius 10.
    let outside = ((50 * w + 64) * 4) as usize;
    assert_eq!(pixels[outside + 3], 0, "outside circle should be transparent, got A={}", pixels[outside + 3]);
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
    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };

    // Paint red circle at center with 50% alpha.
    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0, [255, 0, 0, 128], 1.0);
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
    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };

    let mut enc = encoder(&device);
    target.erase_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let c = ((64 * w + 64) * 4) as usize;

    // Center alpha should be 0 (erased).
    assert_eq!(pixels[c + 3], 0, "center alpha should be 0, got {}", pixels[c + 3]);
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
    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };

    // Composite black → luminance 0 → mask toward 0.
    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0, [0, 0, 0, 255], 1.0);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    let center = (64 * w + 64) as usize;
    assert!(pixels[center] < 10, "center mask should be near 0, got {}", pixels[center]);
    assert_eq!(pixels[0], 255, "corner mask should be 255, got {}", pixels[0]);
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
        &device, &queue, w, h, &sel_data, wgpu::TextureFormat::R8Unorm,
    );

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("test-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let sel_bind_group = pipelines.create_selection_bind_group(&device, &sel_view, &sampler);

    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };

    let mut enc = encoder(&device);
    target.composite_circle_with_selection(
        &mut enc, &pipelines, &queue,
        64.0, 64.0, 30.0,
        [255, 0, 0, 255], 1.0,
        &sel_bind_group,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Left side, within circle (48, 64) — 16px from center, well within r=30.
    let left = ((64 * w + 48) * 4) as usize;
    assert!(pixels[left + 3] > 0, "left (selected) should have paint, A={}", pixels[left + 3]);

    // Right side, within circle (80, 64) — 16px from center, within r=30 but unselected.
    let right = ((64 * w + 80) * 4) as usize;
    assert_eq!(pixels[right + 3], 0, "right (unselected) should be transparent, A={}", pixels[right + 3]);
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
                &pixels[i..i + 4], &[255, 0, 0, 255],
                "pixel ({x},{y}) should be red, got {:?}", &pixels[i..i + 4]
            );
        }
    }
}
