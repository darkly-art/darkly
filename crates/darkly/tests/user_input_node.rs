//! The `user_input` node: one dial an author wires to as many inputs as they
//! like.
//!
//! The graph could already fan a control out by exposing a math node's input
//! and wiring its output around; this node makes that first class. What these
//! tests pin is the node itself, not the settable-source machinery under it:
//! that a dial genuinely reaches several sinks, that the brush bar keeps
//! offering it despite the outgoing wires, that a scrub moves every sink at
//! once, and that it compiles into a brush's shader at all.

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::wire::{BrushWireType, ScalarValue};
use darkly::brush::{nodes::user_input, registry};
use darkly::engine::{DarklyEngine, ExposedValue};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{Graph, NodeId, PortRef};

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 256, 256)
}

/// Install a hand-built graph as the engine's active brush.
fn load(engine: &mut DarklyEngine, graph: &Graph<BrushWireType>) {
    let json = serde_json::to_string(graph).expect("graph serializes");
    engine.set_brush_graph(&json).expect("graph loads");
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

/// `user_input.value` into both `multiply.a` and `add.a`, with the other
/// operands set to their identities so each sink reports the dial verbatim.
fn dial_into_two_sinks() -> (Graph<BrushWireType>, NodeId) {
    let registry = registry();
    let mut graph = Graph::new();

    let ui_reg = registry.get(user_input::TYPE_ID).unwrap();
    let ui = graph.add_node(user_input::TYPE_ID, ui_reg.ports.clone());

    let mul_reg = registry.get("multiply").unwrap();
    let mul = graph.add_node("multiply", mul_reg.ports.clone());
    graph.set_port_default(&mul, "b", 1.0).unwrap();

    let add_reg = registry.get("add").unwrap();
    let add = graph.add_node("add", add_reg.ports.clone());
    graph.set_port_default(&add, "b", 0.0).unwrap();

    wire(&mut graph, (&ui, "value"), (&mul, "a"));
    wire(&mut graph, (&ui, "value"), (&add, "a"));

    (graph, ui)
}

/// Run one dab and read `multiply.result` and `add.result`.
fn both_sinks(graph: &Graph<BrushWireType>) -> (f32, f32) {
    let registry = registry();
    let mut runner =
        BrushGraphRunner::new(graph, registry.as_map(), registry.evaluators()).unwrap();
    runner.seed_sensors(&PaintInformation::default(), [0.0, 0.0, 0.0, 1.0], 42, 0);
    runner.execute_cpu();

    let read = |node: &str| {
        let slot = runner
            .find_output_slot(node, "result")
            .unwrap_or_else(|| panic!("{node}.result has a slot"));
        match runner.read_slot(slot).expect("result has value") {
            ScalarValue::Scalar(v) => v,
            other => panic!("expected Scalar, got {other:?}"),
        }
    };
    (read("multiply"), read("add"))
}

/// The feature: one dial, two sinks, both following it. This is the test that
/// fails against a node that forgot `.source()`, because an ordinary input
/// cannot be wired *from* at all.
#[test]
fn one_dial_drives_two_sinks_on_the_cpu() {
    let (mut graph, ui) = dial_into_two_sinks();

    graph.set_port_default(&ui, "value", 0.37).unwrap();
    let (mul, add) = both_sinks(&graph);
    assert!((mul - 0.37).abs() < 1e-6, "multiply sink read {mul}");
    assert!((add - 0.37).abs() < 1e-6, "add sink read {add}");

    // Moving the one dial moves both sinks together.
    graph.set_port_default(&ui, "value", 0.8).unwrap();
    let (mul, add) = both_sinks(&graph);
    assert!((mul - 0.8).abs() < 1e-6, "multiply sink read {mul}");
    assert!((add - 0.8).abs() < 1e-6, "add sink read {add}");

    // The bar keeps offering the dial despite its two outgoing wires: only an
    // *incoming* wire disqualifies an entry, since that is the case where the
    // artist's scrub would be overwritten by the driver. Without this rule the
    // feature is self-defeating, the act of wiring the dial up would remove it
    // from the brush bar.
    let mut engine = fresh_engine();
    load(&mut engine, &graph);
    let entry = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.key == darkly::nodegraph::exposed_port_key(&ui, "value"))
        .expect("the dial is still an exposed entry");
    match entry.data {
        ExposedValue::Scalar { value, .. } => {
            assert!((value - 0.8).abs() < 1e-6, "bar shows {value}")
        }
        other => panic!("the dial is not a scalar control: {other:?}"),
    }
}

/// The painter's end of the feature: scrubbing the single bar entry moves
/// every sink. Also pins the display mapping, which is identity here because
/// the port is `UnitType::Raw`; a `Percent` registration would store 0.0025.
#[test]
fn scrubbing_the_bar_entry_moves_every_sink() {
    let (graph, ui) = dial_into_two_sinks();
    let mut engine = fresh_engine();
    load(&mut engine, &graph);

    let json = engine
        .brush_set_exposed_port(ui.0.as_str(), "value", 0.25)
        .expect("scrub succeeds");
    let scrubbed: Graph<BrushWireType> = serde_json::from_str(&json).expect("graph json");
    let (mul, add) = both_sinks(&scrubbed);
    assert!((mul - 0.25).abs() < 1e-6, "multiply sink read {mul}");
    assert!((add - 0.25).abs() < 1e-6, "add sink read {add}");

    // The author's value is the brush's identity, so the thumbnail bake must
    // not neutralize it the way it neutralizes an ordinary exposed port. A
    // registration default of 0.5 here would mean every brush carrying a dial
    // bakes a picker icon that misrepresents it.
    let mut for_thumbnail = scrubbed;
    darkly::brush::reset_exposed_scrubs(&mut for_thumbnail);
    let (mul, _) = both_sinks(&for_thumbnail);
    assert!(
        (mul - 0.25).abs() < 1e-6,
        "the dial must survive the thumbnail reset, got {mul}",
    );
}

/// Without `compile_wgsl` the node compiles to "node has no WGSL
/// implementation" and every brush containing one fails to load, which no CPU
/// test catches because the CPU path goes through the generic republish.
#[test]
fn compiled_wgsl_substitutes_the_dial_at_every_use() {
    let registry = registry();
    let mut graph = Graph::new();

    let ui_reg = registry.get(user_input::TYPE_ID).unwrap();
    let ui = graph.add_node(user_input::TYPE_ID, ui_reg.ports.clone());
    graph.set_port_default(&ui, "value", 0.625).unwrap();

    let circle_reg = registry.get("circle").unwrap();
    let circle = graph.add_node("circle", circle_reg.ports.clone());

    let stamp_reg = registry.get("stamp").unwrap();
    let stamp = graph.add_node("stamp", stamp_reg.ports.clone());

    let paint_reg = registry.get("paint").unwrap();
    let paint = graph.add_node("paint", paint_reg.ports.clone());

    // Two uses of the one dial, so the assertion covers substitution at every
    // consuming site rather than just the first.
    wire(&mut graph, (&ui, "value"), (&circle, "softness"));
    wire(&mut graph, (&ui, "value"), (&circle, "aspect"));
    wire(&mut graph, (&circle, "mask"), (&stamp, "tip"));
    wire(&mut graph, (&stamp, "dab"), (&paint, "rgba"));

    let compiled = darkly::brush::compile_graph(&graph)
        .expect("graph compiles")
        .compiled_brush()
        .expect("a graph with a terminal produces a compiled brush");
    assert!(
        compiled.stroke_wgsl.contains("0.625000"),
        "the dial's authored value must reach the shader as a literal",
    );
}
