//! End-to-end test for the stroke recording + replay pipeline.
//!
//! Drives `crates/darkly/src/format/stroke_recording.rs::replay` against
//! a real recording captured from the frontend dev build with
//! `?_RECORD_STROKES=1`. The recording is a short curvy stroke with
//! multiple direction reversals — exactly the workload the stabilizer's
//! "walk back time" rewind path exists to handle. Asserts the parser
//! handles the live wire format and that the replay produces actual
//! paint (non-empty raster bounds) without panicking.

use std::path::PathBuf;

use darkly::brush::builtin_brushes;
use darkly::engine::DarklyEngine;
use darkly::format::stroke_recording::{replay, ReplayPacing, StrokeRecording};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn fixture(name: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect()
}

fn round_brush_graph_json() -> String {
    let brush = builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name.eq_ignore_ascii_case("round"))
        .expect("`round` brush is a built-in");
    serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph")
}

fn build_engine(canvas: (u32, u32)) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, canvas.0, canvas.1)
}

/// Inspects the live wire format produced by the frontend recorder. Any
/// schema drift between the JS encoder and the Rust parser surfaces here.
#[test]
fn live_recording_parses_with_expected_shape() {
    let recording = StrokeRecording::load(&fixture("recorded_curvy_stroke.json"))
        .expect("fixture parses with the live wire format");
    assert_eq!(recording.version, 1);
    assert_eq!(recording.canvas_width, 4000);
    assert_eq!(recording.canvas_height, 2000);
    assert!(
        recording.events.len() > 50,
        "expected a non-trivial number of events, got {}",
        recording.events.len()
    );

    // time_ms must be monotonic non-decreasing — anything else means the
    // recorder mis-ordered events or PointerEvent.timeStamp jumped
    // backward, which would break the replay's pacing math.
    for w in recording.events.windows(2) {
        assert!(
            w[1].time_ms >= w[0].time_ms,
            "time_ms regressed between events: {:?} -> {:?}",
            w[0].time_ms,
            w[1].time_ms,
        );
    }

    // The whole point of recording over synthesizing is that real strokes
    // change direction. A monotonic-in-x diagonal would defeat the test.
    let has_direction_reversal = recording
        .events
        .windows(3)
        .any(|w| (w[1].x - w[0].x).signum() != (w[2].x - w[1].x).signum());
    assert!(
        has_direction_reversal,
        "recording has no x-direction reversal — pick a curvier stroke for this fixture",
    );
}

/// Replays the live recording through a headless engine with the
/// `round` brush. Asserts the harness completes without panicking and
/// that the layer actually accumulated paint.
#[test]
fn replay_produces_paint_on_the_layer() {
    let recording =
        StrokeRecording::load(&fixture("recorded_curvy_stroke.json")).expect("fixture parses");
    // Headless test device runs with `downlevel_defaults` limits (max
    // texture dimension 2048). Layer-storage textures round outward to
    // tile alignment, so the actual GPU texture exceeds the canvas dims
    // by a tile chunk. 1024×512 leaves comfortable headroom under 2048.
    // The `replay` function scales recorded `(x, y)` by
    // `target / recording.canvas_*`, so the stroke covers the same
    // fraction of the canvas regardless of resolution.
    let canvas = (1024, 512);

    let mut engine = build_engine(canvas);
    engine
        .set_brush_graph(&round_brush_graph_json())
        .expect("brush graph compiles");
    let layer_id = engine.add_raster_layer(None);

    let timings = replay(
        &mut engine,
        &recording,
        layer_id,
        canvas,
        // Tests use the back-to-back pacing so CI doesn't pay the 3.5s
        // wall-clock cost the live stroke recorded. The engine's
        // stabilizer reads `time_ms` from each StrokeOp payload (not the
        // wall-clock), so the *engine* behaves identically under either
        // pacing — only the harness's perf numbers differ.
        ReplayPacing::AsFastAsPossible,
    );

    assert_eq!(timings.len(), recording.events.len());
    assert_eq!(timings[0].index, 0);
    assert_eq!(timings.last().unwrap().index, recording.events.len() - 1);

    // After end_stroke + render, the raster layer's stored bounds should
    // be non-empty — the round brush painted somewhere within the canvas.
    let bounds = engine
        .layer_bounds(layer_id)
        .expect("raster layer should report bounds");
    assert!(
        !bounds.is_empty(),
        "expected non-empty raster bounds after replay, got {bounds:?}",
    );
}
