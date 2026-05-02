//! Regression tests for the layer-panel thumbnail reactivity fix.
//!
//! The bug: layer thumbnails in the side panel didn't populate on first
//! load and didn't update after painting — only after Ctrl+Z. Root cause
//! was that the Rust-side thumbnail cache is updated asynchronously by
//! GPU readbacks, but Svelte `$derived` consumers had no signal to
//! re-evaluate when those readbacks completed.
//!
//! These tests cover the engine half of the fix: every pixel-write site
//! must funnel through `compositor.mark_layer_pixels_dirty` so that
//! `engine.render()` auto-queues a thumbnail readback, and the cache
//! must populate via that auto-queue path *without* the test calling
//! `layer_thumbnail()` (which would queue a readback through the legacy
//! path and mask the bug).
//!
//! Critical methodology: assertions go through `test_thumbnail_cache_peek`,
//! NOT `layer_thumbnail`. Calling the latter inside the assertion would
//! make these tests pass on master too — see plan notes for the
//! `git stash` verification step that proves they don't.

use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 256, 256)
}

/// Paint a short brush stroke across the layer at its vertical center.
fn paint_short_stroke(engine: &mut DarklyEngine, layer_id: u64) {
    engine.begin_stroke(layer_id);
    for step in 0..10 {
        engine.stroke_to(StrokeOp::BrushStroke {
            x: step as f32 * 20.0 + 10.0,
            y: 128.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: step as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
}

/// True if any RGBA pixel in `bytes` has non-zero alpha — i.e. the
/// thumbnail captured something other than the all-zero default.
/// `generate_rgba_thumbnail_from_pixels` composites the layer over a
/// checkerboard before storing, but since the layer texture starts all
/// zero (alpha=0), a fresh-empty-layer thumbnail still has alpha=255
/// everywhere; what tells us "real paint landed" is the RGB *not*
/// being the checkerboard's grey-on-grey.
fn has_painted_pixels(bytes: &[u8]) -> bool {
    // Checkerboard fills are 102 or 153 on each RGB channel for an
    // empty layer (see `generate_rgba_thumbnail_from_pixels`). A red
    // brush stroke has cr=1.0, cg=0, cb=0, so the thumbnail will have
    // pixels with R near 255 and G near 0 — well outside the checker
    // values. Loose check: any pixel with R > 200 OR G+B contrast > 50.
    bytes.chunks_exact(4).any(|p| {
        let r = p[0];
        let g = p[1];
        let b = p[2];
        r > 200 || g.abs_diff(b) > 50
    })
}

#[test]
fn paint_stroke_auto_queues_thumbnail_readback() {
    let mut engine = fresh_engine();
    let layer_id = engine.add_raster_layer();

    // Baseline render+flush so any startup readbacks settle. Capture
    // the version *after* settling so the assertion reflects the
    // post-paint delta only.
    engine.render(0.0);
    engine.test_flush_readbacks();
    let v0 = engine.thumbnail_version();

    paint_short_stroke(&mut engine, layer_id);
    engine.render(0.016);
    engine.test_flush_readbacks();

    let v1 = engine.thumbnail_version();
    assert!(
        v1 > v0,
        "thumbnail_version should increase after paint+render+flush; v0={v0} v1={v1}"
    );

    let cached = engine
        .test_thumbnail_cache_peek(layer_id, false)
        .expect("auto-queue path should have populated the layer thumbnail cache");
    assert!(
        !cached.is_empty(),
        "cached thumbnail bytes should be non-empty"
    );
    assert!(
        has_painted_pixels(&cached),
        "cached thumbnail should reflect the painted stroke (saw pure-checkerboard bytes)"
    );
}

#[test]
fn fill_background_auto_queues_thumbnail_readback() {
    let mut engine = fresh_engine();
    let layer_id = engine.add_raster_layer();

    engine.render(0.0);
    engine.test_flush_readbacks();
    let v0 = engine.thumbnail_version();

    engine.fill_background(layer_id);
    engine.render(0.016);
    engine.test_flush_readbacks();

    let v1 = engine.thumbnail_version();
    assert!(
        v1 > v0,
        "thumbnail_version should increase after fill_background; v0={v0} v1={v1}"
    );

    let cached = engine
        .test_thumbnail_cache_peek(layer_id, false)
        .expect("auto-queue path should have populated the layer thumbnail cache");
    assert!(
        !cached.is_empty(),
        "cached thumbnail bytes should be non-empty after fill_background"
    );
}

#[test]
fn undo_auto_queues_thumbnail_readback() {
    let mut engine = fresh_engine();
    let layer_id = engine.add_raster_layer();

    engine.render(0.0);
    engine.test_flush_readbacks();

    paint_short_stroke(&mut engine, layer_id);
    engine.render(0.016);
    engine.test_flush_readbacks();

    let v_after_paint = engine.thumbnail_version();
    let painted = engine
        .test_thumbnail_cache_peek(layer_id, false)
        .expect("paint path populated the cache");
    assert!(
        has_painted_pixels(&painted),
        "post-paint thumbnail should show the stroke"
    );

    engine.undo();
    engine.render(0.032);
    engine.test_flush_readbacks();

    let v_after_undo = engine.thumbnail_version();
    assert!(
        v_after_undo > v_after_paint,
        "thumbnail_version should increase after undo+render+flush; \
         paint={v_after_paint} undo={v_after_undo}"
    );

    let post_undo = engine
        .test_thumbnail_cache_peek(layer_id, false)
        .expect("undo path repopulated the cache");
    assert!(
        !has_painted_pixels(&post_undo),
        "post-undo thumbnail should no longer show the painted stroke \
         — auto-queue must have re-read the restored (empty) layer texture"
    );
}
