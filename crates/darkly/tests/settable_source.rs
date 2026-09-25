//! Settable-source ports + the `brush_settings.size` signal: brush-level guards.
//!
//! Covers the structural invariant across every builtin: the base-size knob
//! is exposed on the `brush_settings` node and never on a terminal.
//!
//! What a brush's base size actually *is* is art, and is not asserted here.
//! A table of per-brush numbers would turn every retune into a test failure
//! while guarding nothing an artist could get wrong: a brush that ships the
//! wrong size is a brush that looks wrong, which no assertion can tell from
//! one that looks right.

use darkly::brush::builtin_brushes;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::nodes::brush_settings;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::registry;
use darkly::brush::wire::ScalarValue;
use darkly::brush::DAB_REFERENCE_SIZE;
use darkly::nodegraph::{Graph, PortRef};

/// Where the base-size knob lives, for every builtin: on `brush_settings`,
/// never on the terminal. Structural, so retuning a brush cannot break it.
#[test]
fn builtins_own_base_size_on_brush_settings() {
    for brush in builtin_brushes::all() {
        let name = brush.metadata.name.clone();
        let graph = &brush.metadata.graph;
        let settings_id = brush_settings::node_id(graph)
            .unwrap_or_else(|| panic!("{name}: builtin must have a brush_settings node"));

        // (a) The size knob is exposed on brush_settings...
        assert!(
            graph.is_port_exposed(&settings_id, "size"),
            "{name}: brush_settings.size must be exposed in the brush bar",
        );

        // ...and NOT on the terminal (the terminal's `size` is unexposed
        // per-touch modulation now).
        let term_id = darkly::brush::find_terminal(graph).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            !graph.is_port_exposed(&term_id, "size"),
            "{name}: the terminal must not expose a size knob",
        );

        // The knob reads a real number, whatever the artist has tuned it to.
        // The value is art; that it exists and is finite is structure.
        let base = brush_settings::base_size(graph);
        assert!(
            base.is_finite() && base > 0.0,
            "{name}: base size must be a positive number, got {base}",
        );
    }
}

/// Wire `brush_settings.size → multiply.a`, set the base size and the `b`
/// factor, run one dab, and read `multiply.result`.
fn size_through_multiply(base_size: f32, b: f32) -> f32 {
    let registry = registry();
    let mut graph = Graph::new();

    let bs_reg = registry.get(brush_settings::TYPE_ID).unwrap();
    let bs = graph.add_node(brush_settings::TYPE_ID, bs_reg.ports.clone());
    graph.set_port_default(&bs, "size", base_size).unwrap();

    let mul_reg = registry.get("multiply").unwrap();
    let mul = graph.add_node("multiply", mul_reg.ports.clone());
    graph.set_port_default(&mul, "b", b).unwrap();

    graph
        .connect(
            PortRef {
                node: bs,
                port: "size".into(),
            },
            PortRef {
                node: mul,
                port: "a".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();
    runner.set_base_size(base_size);
    runner.seed_sensors(&PaintInformation::default(), [0.0, 0.0, 0.0, 1.0], 42, 0);
    runner.execute_cpu();

    let slot = runner.find_output_slot("multiply", "result").unwrap();
    match runner.read_slot(slot).expect("result has value") {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    }
}

/// The `brush_settings.size` graph signal is published in canvas pixels (the
/// brush *diameter* = `base_size * DAB_REFERENCE_SIZE`), so a math node routed
/// between it and a pixel sink operates in the same domain: `× 1` is a true
/// no-op and gains > 1 scale the pixel value directly.
///
/// Regression: before this, `size` published the raw normalized `0..4` knob
/// value, so any math node (or a direct wire into a pixel field like
/// `noise.scale`) received a sub-pixel number that aliased into garbage.
#[test]
fn size_signal_is_brush_diameter_in_pixels() {
    let px = DAB_REFERENCE_SIZE as f32;

    // × 1 is a no-op: the signal is exactly the diameter in pixels.
    assert!((size_through_multiply(0.1, 1.0) - 0.1 * px).abs() < 1e-3);
    assert!((size_through_multiply(0.4, 1.0) - 0.4 * px).abs() < 1e-3);

    // A gain > 1 scales the pixel value (the multiply cap only bounds the
    // manual slider widget, never a wired/authored value).
    assert!((size_through_multiply(0.1, 2.0) - 0.1 * px * 2.0).abs() < 1e-3);

    // Scaling down works too.
    assert!((size_through_multiply(0.2, 0.5) - 0.2 * px * 0.5).abs() < 1e-3);
}
