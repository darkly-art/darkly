//! Helpers shared by the engine-level brush stroke tests.
//!
//! Cargo builds every file under `tests/` as its own crate, so a test file
//! reaches these through `mod common;`. Not every file uses every helper,
//! hence the crate-level `dead_code` allowance.

#![allow(dead_code)]

use std::path::PathBuf;

use darkly::brush::builtin_brushes;
use darkly::brush::input_value::InputValue;
use darkly::coord::CanvasRect;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

pub fn fixture(name: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect()
}

pub fn install_builtin(engine: &mut DarklyEngine, name: &str) {
    let brush = builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == name)
        .unwrap_or_else(|| panic!("{name} is a builtin"));
    let json = serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph");
    engine
        .set_brush_graph(&json)
        .unwrap_or_else(|e| panic!("{name} compiles: {e:?}"));
}

pub fn set_input(engine: &mut DarklyEngine, type_id: &str, port: &str, value: f32) {
    let id = engine
        .active_brush_graph()
        .nodes()
        .values()
        .find(|n| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in active graph"))
        .id
        .0
        .clone();
    engine
        .brush_graph_set_input(&id, port, InputValue::Scalar(value))
        .unwrap_or_else(|e| panic!("{type_id}.{port}: {e:?}"));
}

/// A headless engine with `brush` installed at the given stabilizer
/// strength.
pub fn new_engine(canvas: (u32, u32), brush: &str, stabilize: f32) -> DarklyEngine {
    let (device, queue) = test_device();
    let mut engine = DarklyEngine::new(GpuContext::new_headless(device, queue), canvas.0, canvas.1);
    install_builtin(&mut engine, brush);
    set_input(&mut engine, "brush_settings", "stabilize", stabilize);
    engine
}

pub fn event(x: f32, y: f32, t: f64) -> StrokeOp {
    StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: t,
    }
}

/// `ops` as one stroke with no frame between them, as a headless embedder
/// paints: the pen-up flush renders it whole.
pub fn headless(engine: &mut DarklyEngine, layer: LayerId, ops: &[StrokeOp]) {
    engine.begin_stroke(layer).unwrap();
    for op in ops {
        engine.stroke_to(*op);
    }
    engine.end_stroke();
}

/// The layer's pixels and bounds once every pending GPU readback has landed.
pub fn readback(engine: &mut DarklyEngine, layer: LayerId) -> (Vec<u8>, CanvasRect) {
    engine.test_flush_readbacks();
    let bounds = engine.layer_bounds(layer).expect("raster layer has bounds");
    (engine.test_readback_layer(layer), bounds)
}

/// Two runs' layers, compared byte for byte. `what` names the configuration
/// in the failure message. Returns the layer bounds for the caller's
/// structural assertions.
pub fn assert_same(
    what: &str,
    a_name: &str,
    (a, bounds): (Vec<u8>, CanvasRect),
    b_name: &str,
    (b, b_bounds): (Vec<u8>, CanvasRect),
) -> CanvasRect {
    assert_eq!(
        bounds, b_bounds,
        "{a_name} and {b_name} grew the layer differently ({what})"
    );
    assert!(
        b.as_chunks::<4>().0.iter().any(|px| px[3] > 0),
        "the stroke left the layer empty ({what})"
    );
    if a != b {
        let differing: Vec<(i32, i32, [u8; 4], [u8; 4])> = a
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| {
                let x = bounds.origin.x + (i as u32 % bounds.width) as i32;
                let y = bounds.origin.y + (i as u32 / bounds.width) as i32;
                (x, y, *a, *b)
            })
            .collect();
        panic!(
            "{a_name} differs from {b_name} in {} of {} pixels ({what}, layer {bounds:?}); \
             first few (x, y, {a_name}, {b_name}): {:?}",
            differing.len(),
            b.len() / 4,
            &differing[..differing.len().min(8)]
        );
    }
    bounds
}
