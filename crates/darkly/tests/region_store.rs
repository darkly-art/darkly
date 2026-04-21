//! RegionStore GPU integration tests: save/restore, partial rects, undo/redo, eviction.
//!
//! Run with: `cargo test -p darkly --test region_store`

use darkly::gpu::region_store::RegionStore;
use darkly::gpu::test_utils::*;

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
    assert_eq!(
        &pixels[0..4],
        &[0, 0, 255, 255],
        "should be blue before restore"
    );

    // Restore.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
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
    assert_eq!(
        pixels[0], 180,
        "newest entry should be restorable: expected 180, got {}",
        pixels[0]
    );
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
