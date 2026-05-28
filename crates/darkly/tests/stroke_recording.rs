//! Stroke-recording parser tests. Exercises `StrokeRecording::load` and
//! `RecordedEvent::to_stroke_op` against a fixture identical in shape to
//! what the frontend recorder produces.

use std::path::PathBuf;

use darkly::engine::StrokeOp;
use darkly::format::stroke_recording::{RecordingError, StrokeRecording};

fn fixture_path(name: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect()
}

#[test]
fn loads_sample_recording() {
    let recording = StrokeRecording::load(&fixture_path("stroke_recording_sample.json"))
        .expect("fixture parses");
    assert_eq!(recording.version, 1);
    assert_eq!(recording.canvas_width, 1920);
    assert_eq!(recording.canvas_height, 1080);
    assert_eq!(recording.events.len(), 3);

    let first = &recording.events[0];
    assert_eq!(first.x, 100.0);
    assert_eq!(first.y, 200.0);
    assert_eq!(first.time_ms, 1000.0);

    let last = &recording.events[2];
    assert_eq!(last.pressure, 0.85);
    assert_eq!(last.time_ms, 1032.0);
}

#[test]
fn scales_coordinates_on_conversion_to_stroke_op() {
    let recording = StrokeRecording::load(&fixture_path("stroke_recording_sample.json"))
        .expect("fixture parses");
    let op = recording.events[1].to_stroke_op((2.0, 0.5));
    match op {
        StrokeOp::BrushStroke {
            x,
            y,
            pressure,
            time_ms,
            ..
        } => {
            assert_eq!(x, 300.0);
            assert_eq!(y, 110.0);
            assert_eq!(pressure, 0.55);
            assert_eq!(time_ms, 1016.0);
        }
        _ => panic!("expected BrushStroke variant"),
    }
}

#[test]
fn identity_scale_preserves_coordinates() {
    let recording = StrokeRecording::load(&fixture_path("stroke_recording_sample.json"))
        .expect("fixture parses");
    let op = recording.events[0].to_stroke_op((1.0, 1.0));
    match op {
        StrokeOp::BrushStroke { x, y, .. } => {
            assert_eq!(x, 100.0);
            assert_eq!(y, 200.0);
        }
        _ => panic!("expected BrushStroke variant"),
    }
}

#[test]
fn rejects_unsupported_version() {
    let src = r#"{"version":999,"canvas_width":1,"canvas_height":1,"events":[
        {"x":0.0,"y":0.0,"pressure":0.0,"x_tilt":0.0,"y_tilt":0.0,
         "rotation":0.0,"tangential_pressure":0.0,"time_ms":0.0,
         "cr":0.0,"cg":0.0,"cb":0.0,"ca":1.0}
    ]}"#;
    match StrokeRecording::from_json_str(src) {
        Err(RecordingError::UnsupportedVersion(999)) => {}
        other => panic!("expected UnsupportedVersion(999), got {other:?}"),
    }
}

#[test]
fn rejects_empty_event_list() {
    let src = r#"{"version":1,"canvas_width":1,"canvas_height":1,"events":[]}"#;
    match StrokeRecording::from_json_str(src) {
        Err(RecordingError::Empty) => {}
        other => panic!("expected Empty, got {other:?}"),
    }
}
