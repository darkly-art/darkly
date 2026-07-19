//! Process-recording (timelapse) integration tests: change-triggered capture,
//! throttling with trailing capture, undo-triggered capture, aspect-fit
//! letterboxing, aspect-ratio gating, mid-stroke gating, and backpressure
//! drop semantics.
//!
//! These construct a real `DarklyEngine` via headless `GpuContext` and drive
//! the recorder through `render(t)` + `test_flush_readbacks()` — the same
//! frame loop production uses, minus the surface present.
//! Run with: `cargo test -p darkly --test process_recording -- --test-threads=1`

use darkly::engine::types::StrokeOp;
use darkly::engine::{DarklyEngine, RecordedFrame};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Paint a short brush stroke at (x, y) WITHOUT rendering — the tests
/// control the `render(t)` clock themselves.
fn stroke_at(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32) {
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 1.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
    engine.end_stroke();
}

/// One test frame step: flush GPU readbacks (fires diff-rect + readback map
/// callbacks, dispatching completed readbacks), then run a render tick at
/// time `t`.
fn step(engine: &mut DarklyEngine, t: f32) {
    engine.test_flush_readbacks();
    engine.render(t);
    engine.test_flush_readbacks();
}

/// Drain frames until one arrives or the iteration budget runs out.
fn poll_frame_within(engine: &mut DarklyEngine, t: f32, iters: u32) -> Option<RecordedFrame> {
    for _ in 0..iters {
        step(engine, t);
        if let Some(f) = engine.poll_recording_frame() {
            return Some(f);
        }
    }
    None
}

fn rgba_at(frame: &RecordedFrame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * frame.width + x) * 4) as usize;
    frame.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn paint_stroke_produces_frame_at_configured_dims() {
    let mut engine = test_engine(128, 128);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 128, 128, 128, 128);

    stroke_at(&mut engine, layer, 64.0, 64.0);

    let frame = poll_frame_within(&mut engine, 0.0, 16).expect("frame after paint stroke");
    assert_eq!(frame.width, 128);
    assert_eq!(frame.height, 128);
    assert_eq!(frame.rgba.len(), 128 * 128 * 4);

    // The red stroke at canvas center must be visible in the capture.
    let center = rgba_at(&frame, 64, 64);
    assert!(
        center[0] > 128 && center[3] == 255,
        "expected red-ish opaque pixel at capture center, got {center:?}"
    );
}

#[test]
fn throttle_defers_second_capture_until_trailing_fires() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 2.0, 64, 64, 64, 64);

    // First mutation at t=0.0 captures immediately (no prior capture).
    stroke_at(&mut engine, layer, 16.0, 16.0);
    let first = poll_frame_within(&mut engine, 0.0, 16);
    assert!(first.is_some(), "first change must capture immediately");

    // Second mutation at t=0.5 lands inside the 2.0s window — no frame yet,
    // only a trailing capture armed.
    stroke_at(&mut engine, layer, 48.0, 48.0);
    step(&mut engine, 0.5);
    step(&mut engine, 0.5);
    assert!(
        engine.poll_recording_frame().is_none(),
        "capture inside the throttle window must be deferred"
    );

    // Still inside the window at t=1.0.
    step(&mut engine, 1.0);
    assert!(engine.poll_recording_frame().is_none());

    // The trailing capture fires once the window closes.
    let trailing = poll_frame_within(&mut engine, 2.1, 16);
    assert!(
        trailing.is_some(),
        "trailing capture must fire after the throttle window closes"
    );
}

#[test]
fn forced_capture_fires_without_revision_change() {
    // Live voids stream GPU-authoritative pixels that never bump the document
    // revision, so their visual timeline is invisible to the revision-driven
    // capture path. `request_recording_capture` is the escape hatch that lets
    // the void-milestone sites (first frame, disconnect, stop) record anyway.
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 64, 64, 64, 64);

    // Consume the pending revision from the layer add / stroke so the recorder
    // is caught up: `last_seen_revision == doc.revision`.
    stroke_at(&mut engine, layer, 32.0, 32.0);
    poll_frame_within(&mut engine, 0.0, 16).expect("frame after paint stroke");

    // Baseline: with no document change, nothing is captured.
    for _ in 0..4 {
        step(&mut engine, 1.0);
    }
    assert!(
        engine.poll_recording_frame().is_none(),
        "no capture may fire without a change or a forced request"
    );

    // A forced request captures even though the revision is unchanged.
    engine.request_recording_capture();
    let frame = poll_frame_within(&mut engine, 1.0, 16);
    assert!(
        frame.is_some(),
        "request_recording_capture must fire a capture without a revision change"
    );
}

#[test]
fn forced_capture_bypasses_the_throttle_window() {
    // Milestones (first frame, disconnect, stop) are explicit and rare, so a
    // forced capture must fire immediately even inside the throttle window —
    // otherwise a "final on stop" capture would be deferred up to
    // `min_interval` and missed by the bounded stop-time drain.
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 5.0, 64, 64, 64, 64);

    // First capture at t=0 opens a 5s throttle window.
    stroke_at(&mut engine, layer, 32.0, 32.0);
    poll_frame_within(&mut engine, 0.0, 16).expect("first capture");

    // A forced request at t=0.5 — deep inside the window — must not wait for
    // the window to close (a revision change here would only arm a trailing
    // capture).
    engine.request_recording_capture();
    let frame = poll_frame_within(&mut engine, 0.5, 16);
    assert!(
        frame.is_some(),
        "a forced capture must bypass the throttle window"
    );
}

#[test]
fn undo_triggers_capture() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 64, 64, 64, 64);

    stroke_at(&mut engine, layer, 32.0, 32.0);
    poll_frame_within(&mut engine, 0.0, 16).expect("frame after paint stroke");

    engine.undo();
    let frame = poll_frame_within(&mut engine, 1.0, 16);
    assert!(frame.is_some(), "undo must trigger a capture");
}

#[test]
fn letterbox_pads_wide_canvas_with_black_bars() {
    // 512×256 doc into a 256×256 frame: content occupies rows 64..192,
    // with opaque black bars above and below.
    let mut engine = test_engine(512, 256);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 256, 256, 512, 256);

    stroke_at(&mut engine, layer, 256.0, 128.0);

    let frame = poll_frame_within(&mut engine, 0.0, 16).expect("frame after paint stroke");
    assert_eq!((frame.width, frame.height), (256, 256));

    for (x, y) in [(128, 16), (128, 48), (128, 208), (128, 240)] {
        assert_eq!(
            rgba_at(&frame, x, y),
            [0, 0, 0, 255],
            "letterbox bar at ({x}, {y}) must be opaque black"
        );
    }

    // The red canvas-center stroke maps to the frame center.
    let center = rgba_at(&frame, 128, 128);
    assert!(
        center[0] > 128 && center[3] == 255,
        "expected red-ish opaque pixel at frame center, got {center:?}"
    );
}

#[test]
fn no_capture_while_stroke_active() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 64, 64, 64, 64);

    // Pending revision change (the layer add), but a stroke is in flight —
    // the tick must hold off.
    engine.begin_stroke(layer);
    engine.stroke_to(StrokeOp::BrushStroke {
        x: 32.0,
        y: 32.0,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 1.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
    for _ in 0..4 {
        step(&mut engine, 0.0);
    }
    assert!(
        engine.poll_recording_frame().is_none(),
        "no capture may fire while a stroke is active"
    );

    engine.end_stroke();
    let frame = poll_frame_within(&mut engine, 0.0, 16);
    assert!(frame.is_some(), "capture must fire once the stroke ends");
}

#[test]
fn disable_clears_completed_queue() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 64, 64, 64, 64);

    stroke_at(&mut engine, layer, 32.0, 32.0);

    // Queue several undrained frames, then disable capture. Frames encoded
    // for the old parameters must not survive into the next activation —
    // the frontend would hand them to an encoder configured for different
    // dimensions.
    let mut t = 0.0;
    for _ in 0..4 {
        engine.undo();
        engine.redo();
        t += 0.1;
        step(&mut engine, t);
    }
    engine.set_recording_params(false, 0.0, 0, 0, 0, 0);
    step(&mut engine, t);

    assert!(
        engine.poll_recording_frame().is_none(),
        "disabling the recorder must drop undrained frames"
    );
}

#[test]
fn full_queue_skips_capture_without_consuming_revision() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 64, 64, 64, 64);

    stroke_at(&mut engine, layer, 32.0, 32.0);

    // Fill the completed queue to its bound (4) without draining: each
    // undo/redo pair bumps the revision, and each step captures once.
    let mut t = 0.0;
    for _ in 0..8 {
        engine.undo();
        engine.redo();
        t += 0.1;
        step(&mut engine, t);
    }

    // Queue is full; a fresh revision change must be skipped, not eaten.
    engine.undo();
    t += 0.1;
    step(&mut engine, t);
    step(&mut engine, t);

    // Exactly 4 frames are queued — the bound held.
    let mut drained = 0;
    while engine.poll_recording_frame().is_some() {
        drained += 1;
    }
    assert_eq!(drained, 4, "completed queue must be bounded at 4");

    // The skipped revision was not consumed: with room in the queue, the
    // next tick captures it.
    t += 0.1;
    let frame = poll_frame_within(&mut engine, t, 16);
    assert!(
        frame.is_some(),
        "capture skipped on a full queue must retry after a drain"
    );
}

#[test]
fn ar_change_gates_capture_until_reconfigured() {
    let mut engine = test_engine(128, 128);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 128, 128, 128, 128);

    stroke_at(&mut engine, layer, 64.0, 64.0);
    poll_frame_within(&mut engine, 0.0, 16).expect("frame after paint stroke");

    // Resize to a different aspect ratio: the revision bump must NOT be
    // captured — a frame at the stale aspect would bake letterbox bars into
    // the recording.
    let mut rect = engine.canvas_rect();
    rect.width = 256;
    rect.height = 128;
    engine.resize_canvas(rect);
    for _ in 0..8 {
        step(&mut engine, 1.0);
    }
    assert!(
        engine.poll_recording_frame().is_none(),
        "capture must hold while the canvas aspect differs from the negotiated base"
    );

    // The frontend rolls a new segment: re-negotiated params at the new
    // aspect. The gated revision was retained, so the capture fires without
    // any further document change.
    engine.set_recording_params(true, 0.0, 256, 128, 256, 128);
    let frame =
        poll_frame_within(&mut engine, 2.0, 16).expect("capture must resume after reconfigure");
    assert_eq!((frame.width, frame.height), (256, 128));
}

#[test]
fn same_ar_rescale_does_not_gate() {
    let mut engine = test_engine(128, 128);
    let layer = engine.add_raster_layer(None);
    engine.set_recording_params(true, 0.0, 128, 128, 128, 128);

    stroke_at(&mut engine, layer, 64.0, 64.0);
    poll_frame_within(&mut engine, 0.0, 16).expect("frame after paint stroke");

    // Same aspect ratio at a different scale: capture keeps running at the
    // negotiated frame dims (the aspect-fit viewport absorbs the rescale).
    let mut rect = engine.canvas_rect();
    rect.width = 256;
    rect.height = 256;
    engine.resize_canvas(rect);
    let frame =
        poll_frame_within(&mut engine, 1.0, 16).expect("same-aspect rescale must keep capturing");
    assert_eq!((frame.width, frame.height), (128, 128));
}
