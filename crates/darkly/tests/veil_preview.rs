//! Veil picker preview generation.
//!
//! Verifies the offscreen veil preview renderer (`gpu::veil_preview`) and its
//! engine wiring: animated veils yield a multi-frame loop with visible motion,
//! static veils yield a single frame, and generating a preview never mutates
//! the live veil chain or the document ("without mucking with the main
//! document"). Uses the blocking readback flush (`test_flush_readbacks`) —
//! native-only; the wasm path drains the same `ReadbackScheduler` via the rAF
//! render loop.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::gpu::veil_preview::{ANIMATED_FRAMES, PREVIEW_HEIGHT, PREVIEW_WIDTH};

fn headless_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 256, 256)
}

/// Drive blocking readback flushes until the preview completes, or give up
/// after a generous bound (each animated preview is `ANIMATED_FRAMES` tiny
/// readbacks, all submitted up front).
fn drain_preview(engine: &mut DarklyEngine, type_id: &str) -> Vec<Vec<u8>> {
    for _ in 0..256 {
        if let Some(frames) = engine.poll_veil_preview(type_id) {
            return frames;
        }
        engine.test_flush_readbacks();
    }
    panic!("veil preview for {type_id} never completed");
}

#[test]
fn animated_veil_preview_generates_distinct_frames() {
    let mut engine = headless_engine();
    assert!(
        engine.veil_list().is_empty(),
        "no veils should exist before preview generation"
    );

    engine.start_veil_preview("vhs");
    let frames = drain_preview(&mut engine, "vhs");

    assert_eq!(
        frames.len(),
        ANIMATED_FRAMES as usize,
        "animated veil should produce a full frame loop"
    );
    let expected_len = (PREVIEW_WIDTH * PREVIEW_HEIGHT * 4) as usize;
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f.len(), expected_len, "frame {i} has wrong byte length");
    }

    // Time is advancing between frames → at least one consecutive pair differs.
    let any_motion = frames.windows(2).any(|w| w[0] != w[1]);
    assert!(any_motion, "animated veil frames should differ (motion)");

    // The live veil chain and document are untouched: no veil was added.
    assert!(
        engine.veil_list().is_empty(),
        "preview generation must not mutate the live veil chain"
    );
}

#[test]
fn static_veil_preview_is_single_frame() {
    let mut engine = headless_engine();

    engine.start_veil_preview("monochrome");
    let frames = drain_preview(&mut engine, "monochrome");

    assert_eq!(
        frames.len(),
        1,
        "non-animated veil should produce exactly one frame"
    );
    assert_eq!(
        frames[0].len(),
        (PREVIEW_WIDTH * PREVIEW_HEIGHT * 4) as usize
    );
}

#[test]
fn veil_preview_is_idempotent_once_complete() {
    let mut engine = headless_engine();

    engine.start_veil_preview("monochrome");
    let first = drain_preview(&mut engine, "monochrome");

    // A second start after completion is a no-op: no new readbacks queued,
    // and the cached frames are returned unchanged.
    engine.start_veil_preview("monochrome");
    let second = engine
        .poll_veil_preview("monochrome")
        .expect("frames remain cached after completion");
    assert_eq!(first, second);
}

#[test]
fn unknown_veil_type_is_ignored() {
    let mut engine = headless_engine();
    engine.start_veil_preview("does_not_exist");
    assert!(engine.poll_veil_preview("does_not_exist").is_none());
}
