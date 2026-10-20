//! The `remap` node (Map Range) in a graph: it narrows a `[0, 1]` signal
//! within the normalized range, and the wire remap then carries the result
//! onto the downstream port's natural range, so `random -> remap -> hsv.hue`
//! is a hue jitter whose width is a fraction of a full turn. Its WGSL picks
//! the clamp at compile time from the `clamp` literal. The formula itself is
//! unit-tested beside the node.

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::portable::PortableBrush;
use darkly::brush::wire::{BrushWireType, ScalarValue};
use darkly::brush::{compile_graph, registry};
use darkly::color::hsv_adjust_rgba;
use darkly::nodegraph::{Graph, NodeId, PortRef};

const COLOR: [f32; 4] = [0.8, 0.2, 0.1, 1.0];

fn read(runner: &BrushGraphRunner, type_id: &str, port: &str) -> ScalarValue {
    let slot = runner
        .find_output_slot(type_id, port)
        .unwrap_or_else(|| panic!("{type_id}.{port} has a slot"));
    runner.read_slot(slot).expect("slot has a value")
}

fn read_scalar(runner: &BrushGraphRunner, type_id: &str, port: &str) -> f32 {
    match read(runner, type_id, port) {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    }
}

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

/// `paint_color -> hsv`, `random -> remap (out 0.4..0.6) -> hsv.hue`.
#[test]
fn random_through_remap_into_hue_is_a_narrowed_turn() {
    let registry = registry();
    let mut graph = Graph::new();
    let node = |graph: &mut Graph<BrushWireType>, t: &str| {
        graph.add_node(t, registry.get(t).unwrap().ports.clone())
    };
    let paint_color = node(&mut graph, "paint_color");
    let random = node(&mut graph, "random");
    let remap = node(&mut graph, "remap");
    let hsv = node(&mut graph, "hsv");
    graph.set_port_default(&remap, "out_low", 0.4).unwrap();
    graph.set_port_default(&remap, "out_high", 0.6).unwrap();
    wire(&mut graph, (&paint_color, "color"), (&hsv, "color"));
    wire(&mut graph, (&random, "value"), (&remap, "input"));
    wire(&mut graph, (&remap, "output"), (&hsv, "hue"));

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();
    for dab in 0..8 {
        runner.seed_sensors(&PaintInformation::default(), COLOR, 42, dab);
        runner.clear_slots();
        runner.seed_sensors(&PaintInformation::default(), COLOR, 42, dab);
        runner.execute_cpu();
        let r = read_scalar(&runner, "random", "value");
        let narrowed = read_scalar(&runner, "remap", "output");
        assert!(
            (narrowed - (0.4 + 0.2 * r)).abs() < 1e-5,
            "dab {dab}: remap gave {narrowed} for r = {r}"
        );
        // 0.4..0.6 of the hue port's -180..180 is -36..36 degrees.
        let degrees = -180.0 + 360.0 * narrowed;
        assert!((-36.0 - 1e-3..=36.0 + 1e-3).contains(&degrees));
        let got = match read(&runner, "hsv", "output") {
            ScalarValue::Vec4(v) => v,
            other => panic!("expected Vec4, got {other:?}"),
        };
        let want = hsv_adjust_rgba(COLOR, degrees, 0.0, 0.0);
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4),
            "dab {dab}: got {got:?}, want {want:?}"
        );
    }
}

fn remap_brush_yaml(clamp: bool) -> String {
    format!(
        "name: Remap Test
description: Pressure through a Map Range into flow.
nodes:
  brush_settings:
    type: brush_settings
  circle:
    type: circle
  paint:
    type: paint
  paint_color:
    type: paint_color
  pen_input:
    type: pen_input
  remap:
    type: remap
    inputs:
      clamp: {clamp}
  stamp:
    type: stamp
connections:
- circle.mask -> stamp.tip
- paint_color.color -> stamp.color
- pen_input.position -> paint.position
- pen_input.pressure -> remap.input
- remap.output -> paint.build_flow
- stamp.dab -> paint.rgba
"
    )
}

fn stroke_wgsl(yaml: &str) -> String {
    let portable: PortableBrush = serde_yaml_ng::from_str(yaml).expect("yaml parses");
    let graph = portable.into_graph(registry()).expect("graph builds");
    let runner = compile_graph(&graph).expect("compiles");
    let compiled = runner.compiled_brush().expect("has a terminal");
    let source = compiled.stroke_wgsl.clone();
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}\n{source}", e.emit_to_string(&source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    source
}

#[test]
fn wgsl_clamp_follows_the_literal() {
    let clamped = stroke_wgsl(&remap_brush_yaml(true));
    assert!(
        clamped.contains("clamp(remap_t_remap, 0.0, 1.0)"),
        "{clamped}"
    );
    let free = stroke_wgsl(&remap_brush_yaml(false));
    assert!(free.contains("let remap_t_remap ="), "{free}");
    assert!(!free.contains("clamp(remap_t_remap"), "{free}");
}
