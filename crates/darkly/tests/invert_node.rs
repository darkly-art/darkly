//! The `invert` node: `output = 1 - input` on a wire, the fan-out counterpart
//! of a brush-bar entry's `invert` flag. These pin the complement itself, the
//! no-clamp contract, and the natural-range interaction that distinguishes the
//! node from a `subtract` with `a = 1.0`: a bipolar source is normalized to
//! unit space before the complement. The one-dial-two-sinks use case lives in
//! `user_input_node.rs` beside the other dial tests.

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::registry;
use darkly::brush::wire::{BrushWireType, ScalarValue};
use darkly::nodegraph::{Graph, NodeId, PortRef};

fn wire(graph: &mut Graph<BrushWireType>, from: (&NodeId, &str), to: (&NodeId, &str)) {
    graph
        .connect(
            PortRef {
                node: from.0.clone(),
                port: from.1.into(),
            },
            PortRef {
                node: to.0.clone(),
                port: to.1.into(),
            },
        )
        .expect("wire connects");
}

/// Run one dab seeded from `info` and read `type_id.port`.
fn eval(graph: &Graph<BrushWireType>, info: &PaintInformation, type_id: &str, port: &str) -> f32 {
    let registry = registry();
    let mut runner =
        BrushGraphRunner::new(graph, registry.as_map(), registry.evaluators()).unwrap();
    runner.seed_sensors(info, [0.0, 0.0, 0.0, 1.0], 42, 0);
    runner.execute_cpu();
    let slot = runner
        .find_output_slot(type_id, port)
        .unwrap_or_else(|| panic!("{type_id}.{port} has a slot"));
    match runner.read_slot(slot).expect("slot has a value") {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    }
}

fn invert_of(x: f32) -> f32 {
    let registry = registry();
    let mut graph = Graph::new();
    let node = graph.add_node("invert", registry.get("invert").unwrap().ports.clone());
    graph.set_port_default(&node, "input", x).unwrap();
    eval(&graph, &PaintInformation::default(), "invert", "output")
}

#[test]
fn invert_complements_input() {
    for (x, want) in [(0.0, 1.0), (0.3, 0.7), (1.0, 0.0)] {
        let got = invert_of(x);
        assert!(
            (got - want).abs() < 1e-6,
            "invert({x}) = {got}, want {want}"
        );
    }
}

/// Ranges are UI hints, not bounds: an over-range input complements to an
/// under-range output rather than clamping, matching `subtract` and the
/// brush-bar mirror.
#[test]
fn invert_does_not_clamp() {
    let got = invert_of(1.5);
    assert!((got + 0.5).abs() < 1e-6, "invert(1.5) = {got}, want -0.5");
}

/// `pen_input.x_tilt` declares `-1..1` and `invert.input` declares `0..1`, so
/// the wire remap normalizes the tilt before the complement. A tilt of `0.5`
/// is `0.75` in unit space, complements to `0.25`, and reaches a range-less
/// sink (`add.a`) as that unit value. This is the behaviour a `subtract` with
/// `a = 1.0` cannot give, since `subtract`'s ports carry no natural range.
#[test]
fn bipolar_source_is_normalized_before_the_complement() {
    let registry = registry();
    let mut graph = Graph::new();
    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
    );
    let inv = graph.add_node("invert", registry.get("invert").unwrap().ports.clone());
    let add = graph.add_node("add", registry.get("add").unwrap().ports.clone());
    graph.set_port_default(&add, "b", 0.0).unwrap();
    wire(&mut graph, (&pen, "x_tilt"), (&inv, "input"));
    wire(&mut graph, (&inv, "output"), (&add, "a"));

    let info = PaintInformation {
        x_tilt: 0.5,
        ..Default::default()
    };
    let got = eval(&graph, &info, "add", "result");
    assert!(
        (got - 0.25).abs() < 1e-6,
        "tilt 0.5 inverted read {got}, want 0.25"
    );
}
