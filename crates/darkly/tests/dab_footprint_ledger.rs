//! The dab-footprint ledger: the save-point bbox a dab records must be the
//! footprint its terminal published for the pass it actually issued, never a
//! bbox recomputed from position and radius alongside it.
//!
//! Guards the failure class documented on `ExtentContribution`
//! (`crates/darkly/src/brush/wgsl/extent.rs`): a CPU-side geometric envelope
//! and the shader's real write footprint were maintained independently,
//! disagreed by the compiled brush's extent inflation, and a mid-stroke rewind
//! then cleared pixels outside the CPU bbox while restoring only into it,
//! visibly truncating earlier dabs into a square as the artist kept painting.
//!
//! `smudge` gives a reachable instance. Its `read_half` early-outs on a
//! stationary dab, and `advance_dab_motion` reports zero motion for the first
//! dab of a stroke (there is no previous dab position yet), so that dab issues
//! no pass and writes no pixels. A dab that wrote nothing must claim no damage.
//!
//! Run with: `cargo test -p darkly --test dab_footprint_ledger --features
//! darkly/testing -- --test-threads=1`

use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

const CANVAS: u32 = 256;

fn test_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, CANVAS, CANVAS)
}

fn set_builtin_brush(engine: &mut DarklyEngine, name: &str) {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == name)
        .unwrap_or_else(|| panic!("builtin brush `{name}` not registered"));
    let json = serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph");
    engine.set_brush_graph(&json).expect("brush graph compiles");
}

fn stroke_to(engine: &mut DarklyEngine, x: f32, y: f32, time_ms: f64) {
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms,
        cr: 1.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
}

fn painted_layer(engine: &mut DarklyEngine) -> LayerId {
    let layer_id = engine.add_raster_layer(None);
    // Smudge drags existing pigment; give it something to drag so the
    // stationary early-out is the only reason a dab could write nothing.
    set_builtin_brush(engine, "Ink Pen");
    engine.begin_stroke(layer_id).unwrap();
    stroke_to(engine, 100.0, 128.0, 0.0);
    stroke_to(engine, 156.0, 128.0, 16.0);
    engine.end_stroke();
    engine.render(0.0);
    layer_id
}

/// A dab whose terminal issued no pass must contribute nothing to the
/// cumulative save-point bbox.
///
/// Before the fallback envelope was removed from `StrokeEngine::place_dab`,
/// an unpublished footprint fell back to a `pos ± effective_diameter / 2`
/// rect: a bbox recomputed from geometry, omitting the compiled brush's
/// `brush_extent_factor`, for a dab that wrote no pixels at all.
#[test]
fn dab_that_writes_nothing_records_no_damage() {
    let mut engine = test_engine();
    let layer_id = painted_layer(&mut engine);

    set_builtin_brush(&mut engine, "Smudge");
    engine.begin_stroke(layer_id).unwrap();
    stroke_to(&mut engine, 128.0, 128.0, 0.0);

    let bbox = engine.test_stroke_save_point_bbox();
    assert!(
        bbox.is_none_or(|r| r.is_empty()),
        "smudge's first dab is stationary and issues no pass, so it must \
         record an empty footprint; got {bbox:?}. A non-empty rect here means \
         the save-point bbox was recomputed from position and radius rather \
         than taken from what the terminal published: the divergence that \
         lets a rewind clear pixels it cannot restore."
    );

    engine.end_stroke();
}

/// The complement: once the stroke moves, smudge does issue a pass, and the
/// recorded footprint must be non-empty. Without this the test above would
/// pass just as well against a ledger that never records anything.
#[test]
fn dab_that_writes_records_its_footprint() {
    let mut engine = test_engine();
    let layer_id = painted_layer(&mut engine);

    set_builtin_brush(&mut engine, "Smudge");
    engine.begin_stroke(layer_id).unwrap();
    stroke_to(&mut engine, 100.0, 128.0, 0.0);
    for i in 1..=8 {
        stroke_to(&mut engine, 100.0 + 8.0 * i as f32, 128.0, 16.0 * i as f64);
    }

    let bbox = engine
        .test_stroke_save_point_bbox()
        .expect("a moving smudge stroke places dabs");
    assert!(
        !bbox.is_empty(),
        "a smudge dab that issued a pass must record its write footprint; \
         got an empty rect"
    );

    engine.end_stroke();
}
