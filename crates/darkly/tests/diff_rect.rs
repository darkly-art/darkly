//! The stroke-end diff: tight bounding rect plus per-tile changed flags,
//! decoded into the runs an undo entry stores.
//!
//! Run with: `cargo test -p darkly --features testing --test diff_rect`

use darkly::coord::CanvasRect;
use darkly::gpu::diff_rect::DiffRectPass;
use darkly::gpu::region_runs::RegionRuns;
use darkly::gpu::test_utils::*;

fn cr(x: i32, y: i32, w: u32, h: u32) -> CanvasRect {
    CanvasRect::from_xywh(x, y, w, h)
}

/// Diff two `w × h` textures of `format` that differ in `blobs` (canvas-space
/// rects, with the layer at canvas `origin`) and return the decoded runs.
fn diff_blobs(
    format: wgpu::TextureFormat,
    origin: (i32, i32),
    (w, h): (u32, u32),
    blobs: &[CanvasRect],
) -> RegionRuns {
    let (device, queue) = test_device();
    let bpp = format.block_copy_size(None).unwrap() as usize;
    let a = vec![0u8; w as usize * h as usize * bpp];
    let mut b = a.clone();
    for blob in blobs {
        for y in blob.y0()..blob.y1() {
            for x in blob.x0()..blob.x1() {
                let i = ((y - origin.1) as usize * w as usize + (x - origin.0) as usize) * bpp;
                b[i..i + bpp].fill(255);
            }
        }
    }
    let (_ta, va) = create_test_texture_with_format(&device, &queue, w, h, &a, format);
    let (_tb, vb) = create_test_texture_with_format(&device, &queue, w, h, &b, format);
    let mut pending = DiffRectPass::new(&device).request(
        &device,
        &queue,
        &va,
        &vb,
        cr(origin.0, origin.1, w, h),
        format,
    );
    loop {
        if let Some(result) = pending.poll(&device) {
            return result.expect("the textures differ");
        }
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
}

#[test]
fn separated_blobs_become_their_tiles_clipped_to_the_bbox_rgba() {
    // Layer at canvas (-50, 30). Blob 1 sits in tile (0, 0); blob 2 in tile (4, 2).
    let blobs = [cr(-40, 40, 10, 10), cr(220, 170, 10, 10)];
    let runs = diff_blobs(
        wgpu::TextureFormat::Rgba8Unorm,
        (-50, 30),
        (300, 200),
        &blobs,
    );
    // Tile (0, 0) spans canvas x [-50, 14), y [30, 94); tile (4, 2) spans
    // x [206, 270), y [158, 222), clipped to the bbox (-40, 40)..(230, 180).
    assert_eq!(runs.rects(), &[cr(-40, 40, 54, 54), cr(206, 158, 24, 22)]);
}

#[test]
fn separated_blobs_become_their_tiles_clipped_to_the_bbox_r8() {
    // R8 tiles are 256x64: blob 1 in tile (0, 0), blob 2 in tile (1, 2).
    let blobs = [cr(10, 10, 10, 10), cr(400, 150, 10, 10)];
    let runs = diff_blobs(wgpu::TextureFormat::R8Unorm, (0, 0), (600, 200), &blobs);
    assert_eq!(runs.rects(), &[cr(10, 10, 246, 54), cr(256, 128, 154, 32)]);
}
