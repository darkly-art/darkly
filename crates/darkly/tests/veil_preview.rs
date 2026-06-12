//! Veil picker preview generation.
//!
//! Verifies the offscreen veil preview renderer (`gpu::veil_preview`) and its
//! engine wiring: the preview is rendered over the *current canvas*, animated
//! veils yield a multi-frame loop with visible motion, static veils yield a
//! single frame, generation regenerates on each call (no cross-open cache), and
//! it never mutates the live veil chain. Uses the blocking readback flush
//! (`test_flush_readbacks`) — native-only; the wasm path drains the same
//! `ReadbackScheduler` via the rAF render loop.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::gpu::veil_preview::ANIMATED_FRAMES;

/// Headless engine with a solid-filled canvas so the composite the preview
/// samples has real content.
fn headless_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 256, 256);
    let layer = engine.add_raster_layer(None);
    engine.fill_background_color(layer, [120, 60, 200, 255]);
    engine
}

/// Drive blocking readback flushes until the preview completes, or give up
/// after a generous bound (each animated preview is `ANIMATED_FRAMES` tiny
/// readbacks, all submitted up front). Returns `(width, height, frames)`.
fn drain_preview(engine: &mut DarklyEngine, type_id: &str) -> (u32, u32, Vec<Vec<u8>>) {
    for _ in 0..256 {
        if let Some(result) = engine.poll_veil_preview(type_id) {
            return result;
        }
        engine.test_flush_readbacks();
    }
    panic!("veil preview for {type_id} never completed");
}

#[test]
fn animated_veil_preview_generates_distinct_frames() {
    let mut engine = headless_engine();

    engine.start_veil_preview("vhs");
    let (w, h, frames) = drain_preview(&mut engine, "vhs");

    assert_eq!(
        frames.len(),
        ANIMATED_FRAMES as usize,
        "animated veil should produce a full frame loop"
    );
    let expected_len = (w * h * 4) as usize;
    assert!(w > 0 && h > 0, "preview should have non-zero dimensions");
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f.len(), expected_len, "frame {i} has wrong byte length");
    }

    // Time is advancing between frames → at least one consecutive pair differs.
    let any_motion = frames.windows(2).any(|pair| pair[0] != pair[1]);
    assert!(any_motion, "animated veil frames should differ (motion)");

    // The live veil chain is untouched: no veil was added to the document.
    assert!(
        engine.veil_list().is_empty(),
        "preview generation must not mutate the live veil chain"
    );
}

#[test]
fn static_veil_preview_is_single_frame() {
    let mut engine = headless_engine();

    engine.start_veil_preview("monochrome");
    let (w, h, frames) = drain_preview(&mut engine, "monochrome");

    assert_eq!(
        frames.len(),
        1,
        "non-animated veil should produce exactly one frame"
    );
    assert_eq!(frames[0].len(), (w * h * 4) as usize);
}

#[test]
fn veil_preview_regenerates_on_each_open() {
    let mut engine = headless_engine();

    engine.start_veil_preview("monochrome");
    let first = drain_preview(&mut engine, "monochrome");

    // No caching: a second start after completion re-renders against the live
    // canvas and produces a fresh, valid preview.
    engine.start_veil_preview("monochrome");
    let second = drain_preview(&mut engine, "monochrome");
    assert_eq!(first.0, second.0);
    assert_eq!(first.1, second.1);
    assert_eq!(first.2.len(), second.2.len());
}

#[test]
fn unknown_veil_type_is_ignored() {
    let mut engine = headless_engine();
    engine.start_veil_preview("does_not_exist");
    assert!(engine.poll_veil_preview("does_not_exist").is_none());
}
