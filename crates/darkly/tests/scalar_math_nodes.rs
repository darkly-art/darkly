//! CPU-evaluation tests for the scalar binary-math nodes (add, subtract,
//! multiply). They share one implementation ([`darkly::brush::scalar_binary`]),
//! so these pin the per-operator behavior — the operator token and identity
//! default that distinguish the three.

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::registry;
use darkly::brush::wire::ScalarValue;
use darkly::nodegraph::Graph;

/// Build a single-node graph of `type_id` with `a`/`b` set to constants,
/// evaluate one dab on the CPU, and return `result`.
fn eval_binary(type_id: &str, a: f32, b: f32) -> f32 {
    let registry = registry();
    let mut graph = Graph::new();
    let reg = registry
        .get(type_id)
        .unwrap_or_else(|| panic!("no registration for {type_id}"));
    let node = graph.add_node(type_id, reg.ports.clone());
    graph.set_port_default(&node, "a", a).unwrap();
    graph.set_port_default(&node, "b", b).unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();
    let info = PaintInformation::default();
    runner.seed_sensors(&info, [0.0, 0.0, 0.0, 1.0], 42, 0);
    runner.execute_cpu();

    let slot = runner
        .find_output_slot(type_id, "result")
        .expect("result slot");
    match runner.read_slot(slot).expect("result has value") {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    }
}

#[test]
fn add_sums_inputs() {
    assert!((eval_binary("add", 0.3, 0.4) - 0.7).abs() < 1e-6);
}

#[test]
fn subtract_differences_inputs() {
    assert!((eval_binary("subtract", 0.9, 0.4) - 0.5).abs() < 1e-6);
}

#[test]
fn multiply_products_inputs() {
    assert!((eval_binary("multiply", 0.5, 0.5) - 0.25).abs() < 1e-6);
}

/// Each operator's identity element is both ports' default, so an
/// unconnected node is a no-op: `a` alone passes through unchanged.
#[test]
fn identity_defaults_pass_a_through() {
    // add/subtract identity is 0 (a + 0 == a - 0 == a); multiply is 1.
    assert!((eval_binary_a_only("add", 0.6) - 0.6).abs() < 1e-6);
    assert!((eval_binary_a_only("subtract", 0.6) - 0.6).abs() < 1e-6);
    assert!((eval_binary_a_only("multiply", 0.6) - 0.6).abs() < 1e-6);
}

/// Set only `a`, leaving `b` at its registration default (the identity).
fn eval_binary_a_only(type_id: &str, a: f32) -> f32 {
    let registry = registry();
    let mut graph = Graph::new();
    let reg = registry.get(type_id).unwrap();
    let node = graph.add_node(type_id, reg.ports.clone());
    graph.set_port_default(&node, "a", a).unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();
    let info = PaintInformation::default();
    runner.seed_sensors(&info, [0.0, 0.0, 0.0, 1.0], 42, 0);
    runner.execute_cpu();

    let slot = runner.find_output_slot(type_id, "result").unwrap();
    match runner.read_slot(slot).unwrap() {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    }
}
