//! Void picker preview generation.
//!
//! Verifies the offscreen void preview renderer (`gpu::void_preview`) and its
//! engine wiring: a void that opts into previews (`noise`) renders a thumbnail
//! from scratch at the canvas's aspect-fit size, captured as readable RGBA
//! frames, generated through the same shared `previews` map / `poll_preview`
//! path as veils. Uses the blocking readback flush (`test_flush_readbacks`) —
//! native-only; the wasm path drains the same `ReadbackScheduler` via the rAF
//! render loop.

use darkly::engine::{DarklyEngine, PreviewKind};
use darkly::gpu::context::GpuContext;
use darkly::gpu::preview::fit_preview_dims;
use darkly::gpu::test_utils::test_device;

fn headless_engine(w: u32, h: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, w, h)
}

/// Drive blocking readback flushes until the preview completes, or give up after
/// a generous bound. Returns `(width, height, frames)`.
fn drain_preview(engine: &mut DarklyEngine, type_id: &str) -> (u32, u32, Vec<Vec<u8>>) {
    for _ in 0..256 {
        if let Some(result) = engine.poll_preview(PreviewKind::Void, type_id) {
            return result;
        }
        engine.test_flush_readbacks();
    }
    panic!("void preview for {type_id} never completed");
}

#[test]
fn noise_void_preview_generates_a_frame() {
    let mut engine = headless_engine(256, 256);

    engine.start_void_preview("noise");
    let (w, h, frames) = drain_preview(&mut engine, "noise");

    // Noise is a static void (default `needs_animation()` is false) → one frame.
    assert_eq!(
        frames.len(),
        1,
        "static void should produce exactly one frame"
    );

    // Dimensions are the canvas aspect-fit thumbnail size.
    let (pw, ph) = fit_preview_dims(256, 256);
    assert_eq!(
        (w, h),
        (pw, ph),
        "preview dims should aspect-fit the canvas"
    );
    assert!(w > 0 && h > 0, "preview should have non-zero dimensions");
    assert_eq!(
        frames[0].len(),
        (w * h * 4) as usize,
        "frame is tightly packed RGBA8"
    );

    // The noise field renders real content — not a blank buffer.
    assert!(
        frames[0].iter().any(|&b| b != 0),
        "noise preview frame should contain rendered (non-zero) pixels"
    );

    // The live layer stack is untouched: no void layer was added to the doc.
    assert!(
        engine.layer_tree().is_empty(),
        "preview generation must not mutate the live layer stack"
    );
}

#[test]
fn non_square_canvas_preview_fits_aspect() {
    // A wide canvas keeps its aspect, capped on the long edge by PREVIEW_MAX_DIM.
    let mut engine = headless_engine(800, 400);
    engine.start_void_preview("noise");
    let (w, h, _) = drain_preview(&mut engine, "noise");
    let (pw, ph) = fit_preview_dims(800, 400);
    assert_eq!((w, h), (pw, ph));
    assert!(w > h, "wide canvas should yield a wide preview");
}

#[test]
fn unknown_void_type_is_ignored() {
    let mut engine = headless_engine(64, 64);
    engine.start_void_preview("does_not_exist");
    assert!(engine
        .poll_preview(PreviewKind::Void, "does_not_exist")
        .is_none());
}
