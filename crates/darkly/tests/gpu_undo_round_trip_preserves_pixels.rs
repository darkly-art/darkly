//! Regression test for the GPU undo-storage corruption bug.
//!
//! On master, [`RegionStore`] kept a 256 MB ring buffer of undo bytes that
//! evicted on its own schedule, silently invalidating [`UndoRegionEntry`]
//! values the undo stack still held. After enough commits to wrap the ring,
//! the canvas reverted to a glitchy / squished pattern instead of the saved
//! pre-stroke state.
//!
//! This test drives full-canvas commits past the ring capacity and asserts
//! that an undo all the way back to the original state recovers the original
//! pixels byte-for-byte. On master it fails (corrupted pixels); on the
//! per-entry redesign it passes.
//!
//! Run with: `cargo test -p darkly --test gpu_undo_round_trip_preserves_pixels -- --test-threads=1`

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Paint 20 full-canvas commits across a 2048² canvas (~320 MB of undo
/// pixels — well past the legacy 256 MB ring), then undo all 20 times.
/// The final canvas must match the initial post-A state pixel-for-pixel.
#[test]
fn gpu_undo_round_trip_preserves_pixels() {
    // 2048² RGBA = 16 MB per commit. 20 commits = 320 MB, which wraps the
    // legacy 256 MB ring (the bug surfaces on commit 17 and later).
    let (w, h) = (2048, 2048);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    // Anchor state A — solid red across the layer. This is the snapshot
    // every subsequent fill's `save_region` captures, so it's also what the
    // final undo step must reproduce.
    engine.fill_background_color(layer, [255, 0, 0, 255]);
    engine.render(0.0);
    engine.test_flush_readbacks();
    let pixels_a = engine.test_readback_layer(layer);

    // 20 distinctive successor fills. Each commits a fresh 16 MB undo
    // entry. The legacy ring wraps somewhere around commit 17.
    for i in 0..20u8 {
        let color = [i.wrapping_mul(12), 200, 0, 255];
        engine.fill_background_color(layer, color);
        engine.render(0.0);
        engine.test_flush_readbacks();
    }

    // Undo all 20 fills. After the last undo the layer must equal
    // pixels_a — every intermediate restore must produce valid pixels for
    // the round-trip to land correctly.
    for _ in 0..20 {
        engine.undo();
        engine.render(0.0);
        engine.test_flush_readbacks();
    }

    let pixels_after = engine.test_readback_layer(layer);
    assert_eq!(
        pixels_a.len(),
        pixels_after.len(),
        "readback lengths must match"
    );
    let mismatches = pixels_a
        .iter()
        .zip(pixels_after.iter())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        mismatches,
        0,
        "post-undo canvas must equal the saved pre-fill state: {mismatches} mismatched bytes \
         out of {} ({}%)",
        pixels_a.len(),
        (mismatches as f64 / pixels_a.len() as f64) * 100.0
    );
}
