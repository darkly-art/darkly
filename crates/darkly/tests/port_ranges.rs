//! Author-declared slider ranges: per-instance `PortDef::min`/`max`.
//!
//! A brush author can re-range any input port for one brush, so a control
//! whose registration range is a poor fit (a math node's hardcoded `0..1`
//! standing in for a bipolar knob, or a useful band occupying a sliver of
//! the declared range) becomes usable without a helper node in the graph
//! doing the arithmetic. The range lives on the instance port, so the
//! brush bar and the node editor both see it.

use darkly::brush::builtin_brushes;
use darkly::engine::{DarklyEngine, ExposedValue};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{NodeId, PortDir};

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 256, 256)
}

/// Read the `(min, max, value)` the brush bar would render for an exposed
/// scalar control, by label.
fn scalar_control(engine: &DarklyEngine, label: &str) -> (f32, f32, f32) {
    let info = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.label == label)
        .unwrap_or_else(|| panic!("no exposed control labelled '{label}'"));
    match info.data {
        ExposedValue::Scalar {
            value, min, max, ..
        } => (min, max, value),
        other => panic!("'{label}' is not a scalar control: {other:?}"),
    }
}

/// The end-to-end path the feature exists for: a range set through the
/// engine handler reaches the brush bar's reported bounds, and the port's
/// authored value is left alone by the re-range.
#[test]
fn declared_range_reaches_the_brush_bar() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    let (min, max, _) = scalar_control(&engine, "Twirl");
    assert_eq!(
        (min, max),
        (-1.0, 1.0),
        "Hair's Twirl declares a bipolar range in its yaml"
    );

    // Re-range it through the handler and confirm the brush bar follows.
    engine
        .brush_graph_set_port_range("multiply_2", "a", -4.0, 4.0)
        .expect("re-range succeeds");
    let (min, max, value) = scalar_control(&engine, "Twirl");
    assert_eq!((min, max), (-4.0, 4.0));
    assert!(
        (value - 0.5).abs() < 1e-6,
        "re-ranging must not disturb the authored value, got {value}"
    );
}

/// Bounds are UI hints; a degenerate or inverted one breaks the normalize
/// and clamp arithmetic every slider does, so the handler rejects them
/// rather than letting a broken control reach the bar.
#[test]
fn engine_rejects_degenerate_and_inverted_ranges() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    for (min, max) in [(1.0_f32, 1.0_f32), (1.0, -1.0)] {
        assert!(
            engine
                .brush_graph_set_port_range("multiply_2", "a", min, max)
                .is_err(),
            "({min}, {max}) should be rejected"
        );
    }
    // The original range survived every rejection.
    assert_eq!(scalar_control(&engine, "Twirl").0, -1.0);
}

/// The handler's numbers are display-space, the storage is raw. Without the
/// conversion a `Percent` port's declared range drifts by 100× on every
/// save/reload cycle, which is invisible until a brush is reopened.
#[test]
fn percent_port_range_round_trips_through_display_space() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    // `brush_settings.stabilize` is declared `UnitType::Percent`, so a
    // display range of 0-50% must land as a raw 0.0-0.5.
    let json = engine
        .brush_graph_set_port_range("brush_settings", "stabilize", 0.0, 50.0)
        .expect("re-range succeeds");

    let graph: serde_json::Value = serde_json::from_str(&json).expect("graph json");
    let port = graph["nodes"]["brush_settings"]["ports"]
        .as_array()
        .expect("ports array")
        .iter()
        .find(|p| p["name"] == "stabilize")
        .expect("stabilize port");
    assert_eq!(port["min"].as_f64().unwrap(), 0.0);
    assert_eq!(
        port["max"].as_f64().unwrap(),
        0.5,
        "display 50% must store as raw 0.5"
    );

    // And it comes back out in the space it went in.
    let (min, max, _) = scalar_control(&engine, "Stabilize");
    assert_eq!((min, max), (0.0, 50.0));
}

/// The Hair conversion: both controls that used to need a helper node are
/// now plain exposed ports carrying a declared range.
///
/// The Twirl assertion is the real invariant of the conversion. Rotation is
/// `(distance/size) × multiply.b × multiply_2.a`, so replacing the
/// `subtract`-recentered `0..1` control with a bipolar one required halving
/// `multiply.b`. The product of the two is what must be conserved: check
/// it, not the two literals separately.
#[test]
fn hair_expresses_both_controls_without_helper_nodes() {
    let hair = builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Hair")
        .expect("Hair builtin exists");
    let graph = &hair.metadata.graph;

    let port = |node: &str, port: &str| {
        graph
            .nodes()
            .get(&NodeId(node.into()))
            .unwrap_or_else(|| panic!("Hair has a '{node}' node"))
            .ports
            .iter()
            .find(|p| p.name == port && p.dir == PortDir::Input)
            .unwrap_or_else(|| panic!("'{node}' has an input '{port}'"))
    };

    // Twirl: bipolar control, and the rotation coefficient is conserved
    // against the pre-conversion `0.25 × 5.12`.
    let twirl = port("multiply_2", "a");
    assert_eq!((twirl.min, twirl.max), (-1.0, 1.0));
    let coefficient = twirl.value.as_f32() * port("multiply", "b").value.as_f32();
    assert!(
        (coefficient - 1.28).abs() < 1e-5,
        "twirl coefficient drifted: {coefficient}"
    );

    // Hair Thickness: the slider spans the usable band directly, and its
    // stored value is the midpoint the curve node used to produce.
    let thickness = port("multiply_3", "b");
    assert!((thickness.min - 0.03296951).abs() < 1e-7);
    assert!((thickness.max - 0.21059628).abs() < 1e-7);
    assert!((thickness.value.as_f32() - 0.1217829).abs() < 1e-6);

    // Neither control routes through a helper node any more: `multiply_3.b`
    // and `multiply_2.a` are unwired, which is also what keeps them
    // user-scrubbable at all.
    for (node, port_name) in [("multiply_3", "b"), ("multiply_2", "a")] {
        assert!(
            !graph
                .connections
                .iter()
                .any(|c| c.to.node.0 == node && c.to.port == port_name),
            "{node}.{port_name} should be driven by the user, not a wire"
        );
    }

    // And the two workaround nodes are gone, not merely bypassed.
    assert_eq!(
        graph
            .nodes()
            .values()
            .filter(|n| n.type_id == "curve" || n.type_id == "subtract")
            .count(),
        2,
        "Hair should keep only its pressure curve and its noise subtract"
    );
}

/// Every builtin's declared ranges are well-formed. This is the guard that
/// makes the yaml key safe to hand to brush authors: a typo'd range fails
/// the suite instead of shipping a control that can't be dragged.
#[test]
fn every_builtin_declares_sane_ranges() {
    for brush in builtin_brushes::all() {
        let name = &brush.metadata.name;
        for node in brush.metadata.graph.nodes().values() {
            for port in node.ports.iter().filter(|p| p.dir == PortDir::Input) {
                assert!(
                    port.min.is_finite() && port.max.is_finite() && port.min < port.max,
                    "{name}: {}.{} has range ({}, {})",
                    node.type_id,
                    port.name,
                    port.min,
                    port.max
                );
            }
        }
    }
}
