//! Every shipped brush actually paints.
//!
//! The suite checks a great deal about the engine and almost nothing about
//! the fourteen brushes, deliberately: their tuning is art, and tests that
//! pin art break every time an artist changes their mind. This is the floor
//! under that, and the only thing here that names shipped brushes at all.
//!
//! A brush that assembles and compiles can still deposit nothing, through a
//! wire left dangling or a flow authored at zero, and nothing else would
//! notice. So: stroke each one and require the layer to move. The assertion
//! is deliberately the weakest one that still catches a dead brush, because
//! anything stronger is a statement about how the brush looks.
//!
//! Which brushes transport rather than deposit is asked of the graph
//! (`preview_backdrop`), never of a hardcoded name list, so adding or
//! retiring a brush needs no edit here.

use darkly::brush::portable::PortableBrush;
use darkly::brush::{builtin_brushes, graph_capabilities};
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::preview::PreviewBackdrop;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

const W: u32 = 128;
const H: u32 = 64;

/// The test's own vehicle, used to lay down the contrast a sampling brush
/// needs. Blurring a uniform field is correctly a no-op, so a flat fill would
/// fail Blur for the right reason and tell us nothing.
const ANALYTIC_DISC: &str = include_str!("fixtures/analytic_disc.yaml");

fn test_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    DarklyEngine::new(GpuContext::new_headless(device, queue), W, H)
}

/// Fill `layer` opaque, so a brush that samples or moves existing pixels has
/// something to work with.
fn fill(engine: &mut DarklyEngine, layer: LayerId, r: u8, g: u8, b: u8) {
    engine.begin_stroke(layer).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r,
        g,
        b,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
}

fn install(engine: &mut DarklyEngine, graph_json: &str, what: &str) {
    engine
        .set_brush_graph(graph_json)
        .unwrap_or_else(|e| panic!("{what}: graph compiles: {e:?}"));
}

/// Lay thin vertical bars across the layer with the fixture, so a brush that
/// samples or displaces its surroundings has structure to act on.
///
/// Vertical and thin on purpose. The stroke under test runs horizontally, so
/// it crosses every bar and there is contrast under its dab along the whole
/// path. Bands that merged into a solid field would make Blur a no-op for
/// the right reason and tell us nothing about the brush.
fn stripe(engine: &mut DarklyEngine, layer: LayerId) {
    let portable: PortableBrush = serde_yaml_ng::from_str(ANALYTIC_DISC).expect("fixture parses");
    let mut graph = portable
        .into_graph(darkly::brush::registry())
        .expect("fixture builds");
    let mul = graph
        .nodes()
        .values()
        .find(|n| n.type_id == "multiply")
        .expect("fixture has a multiply")
        .id
        .clone();
    // Full flow, so the bars are opaque and the contrast is unambiguous.
    graph
        .set_port_value(
            &mul,
            "b",
            darkly::brush::input_value::InputValue::Scalar(1.0),
        )
        .expect("multiply factor port");
    // Thin, so the bars stay separated by clear ground.
    let settings = graph
        .nodes()
        .values()
        .find(|n| n.type_id == "brush_settings")
        .expect("fixture has brush_settings")
        .id
        .clone();
    graph
        .set_port_value(
            &settings,
            "size",
            darkly::brush::input_value::InputValue::Scalar(0.01),
        )
        .expect("brush_settings size port");
    let json = serde_json::to_string(&graph).expect("serialize graph");
    install(engine, &json, "the fixture");
    for x in (8..W - 8).step_by(8) {
        engine.begin_stroke(layer).unwrap();
        for i in 0..12 {
            engine.stroke_to(StrokeOp::BrushStroke {
                x: x as f32,
                y: 4.0 + i as f32 * ((H as f32 - 8.0) / 12.0),
                pressure: 1.0,
                x_tilt: 0.0,
                y_tilt: 0.0,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: i as f64 * 16.0,
                cr: 0.0,
                cg: 0.0,
                cb: 0.0,
                ca: 1.0,
            });
        }
        engine.end_stroke();
    }
    engine.test_flush_readbacks();
}

/// The stroke under test: white, so it contrasts with both the light ground
/// and the dark bands. A brush painting the colour already under it would
/// deposit correctly and change nothing.
fn stroke(engine: &mut DarklyEngine, layer: LayerId) {
    stroke_at(engine, layer, (H / 2) as f32, [1.0, 1.0, 1.0]);
}

fn stroke_at(engine: &mut DarklyEngine, layer: LayerId, y: f32, rgb: [f32; 3]) {
    engine.begin_stroke(layer).unwrap();
    for i in 0..24 {
        engine.stroke_to(StrokeOp::BrushStroke {
            x: 8.0 + i as f32 * ((W as f32 - 16.0) / 24.0),
            y,
            pressure: 0.8,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
            cr: rgb[0],
            cg: rgb[1],
            cb: rgb[2],
            ca: 1.0,
        });
    }
    engine.end_stroke();
    engine.test_flush_readbacks();
}

#[test]
fn every_builtin_deposits_or_moves_pixels() {
    for brush in builtin_brushes::all() {
        let name = brush.metadata.name.clone();
        let mut engine = test_engine();
        let layer = engine.add_raster_layer(None);

        // Opaque ground with structure on it. A transporting brush has
        // something to pick up or smear, and a depositing brush's black
        // stroke is visible against the light bands.
        fill(&mut engine, layer, 200, 200, 200);
        stripe(&mut engine, layer);
        let before = engine.test_readback_layer(layer);

        let json = serde_json::to_string(&brush.metadata.graph).expect("serialize graph");
        install(&mut engine, &json, &name);

        // A cloning brush has nothing to copy until the painter anchors a
        // source, and would deposit nothing without one. Set one for every
        // brush rather than asking which brushes clone: a brush whose graph
        // has no `clone_source` node simply ignores it, so this needs no
        // edit when a brush is added.
        engine.set_clone_source(16.0, 8.0, Some(layer));

        stroke(&mut engine, layer);
        let after = engine.test_readback_layer(layer);

        let moved = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        let kind = match graph_capabilities(&brush.metadata.graph).preview_backdrop {
            PreviewBackdrop::Stripes => "samples the canvas",
            _ => "deposits pigment",
        };
        assert!(
            moved > 0,
            "'{name}' {kind} but a stroke across the layer changed nothing"
        );
    }
}
