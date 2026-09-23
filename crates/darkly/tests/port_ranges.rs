//! The authored display mapping for an exposed control: the per-instance
//! range (`PortDef::min`/`max`), the per-entry mirror
//! (`ExposedPortMeta::invert`) and the per-entry unit
//! (`ExposedPortMeta::unit`).
//!
//! A brush author can re-range any input port for one brush, so a control
//! whose registration range is a poor fit (a math node's hardcoded `0..1`
//! standing in for a bipolar knob, or a useful band occupying a sliver of
//! the declared range) becomes usable without a helper node in the graph
//! doing the arithmetic. The range lives on the instance port, so the
//! brush bar and the node editor both see it.
//!
//! `invert` is the other half of the same move: it reverses which way the
//! control reads, deleting the `1 - x` helper node an author would
//! otherwise wire in to expose a softness port as a hardness knob. Unlike
//! the range it is brush-bar only, because a mirrored number under the node
//! editor's registration label would simply be wrong.
//!
//! The unit override is the third: it changes which unit the control reads
//! in, so one dial can be authored as degrees or percent whatever its
//! registration declares. Like the mirror it is display-only and brush-bar
//! only, and like the mirror it converts on the way in as well as out.

use darkly::brush::builtin_brushes;
use darkly::engine::{DarklyEngine, ExposedValue};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{ExposedPortMeta, NodeId, PortDir};
use darkly::units::UnitType;

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

/// The raw stored value of an input port, straight out of the graph JSON:
/// the other side of every display-space assertion here.
fn stored_value(json: &str, node: &str, port_name: &str) -> f64 {
    let graph: serde_json::Value = serde_json::from_str(json).expect("graph json");
    graph["nodes"][node]["ports"]
        .as_array()
        .expect("ports array")
        .iter()
        .find(|p| p["name"] == port_name)
        .unwrap_or_else(|| panic!("no port '{port_name}' on node '{node}'"))["value"]
        .as_f64()
        .expect("scalar port value")
}

/// Rewrite one entry's meta the way the authoring modal does: the whole
/// bundle at once, carrying the entry's current label / description / icon
/// back so the edit doesn't clear them.
fn set_meta(
    engine: &mut DarklyEngine,
    key: &str,
    edit: impl FnOnce(&mut ExposedPortMeta),
) -> String {
    let info = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.key == key)
        .unwrap_or_else(|| panic!("no exposed entry keyed '{key}'"));
    let mut meta = ExposedPortMeta {
        label: info.label,
        description: info.description,
        icon: info.icon,
        ..Default::default()
    };
    if let ExposedValue::Scalar {
        invert,
        unit_override,
        ..
    } = info.data
    {
        meta.invert = invert;
        meta.unit = unit_override;
    }
    edit(&mut meta);
    engine
        .brush_graph_set_exposed_port_meta(key, meta)
        .expect("meta write succeeds")
}

fn set_invert(engine: &mut DarklyEngine, key: &str, invert: bool) -> String {
    set_meta(engine, key, |m| m.invert = invert)
}

fn set_unit(engine: &mut DarklyEngine, key: &str, unit: Option<UnitType>) -> String {
    set_meta(engine, key, |m| m.unit = unit)
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

/// Bounds are stored verbatim, whatever unit the control reads in. The
/// handler is unit-blind on purpose: it is what lets a caller write a unit
/// override and a range in either order without the second write being
/// interpreted in the wrong unit.
#[test]
fn port_range_is_stored_verbatim_whatever_the_unit() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    // `brush_settings.stabilize` is declared `UnitType::Percent`, so a
    // stored 0.0-0.5 is the 0-50% band the artist sees.
    let bounds = |json: &str| {
        let graph: serde_json::Value = serde_json::from_str(json).expect("graph json");
        let port = graph["nodes"]["brush_settings"]["ports"]
            .as_array()
            .expect("ports array")
            .iter()
            .find(|p| p["name"] == "stabilize")
            .expect("stabilize port")
            .clone();
        (port["min"].as_f64().unwrap(), port["max"].as_f64().unwrap())
    };

    let json = engine
        .brush_graph_set_port_range("brush_settings", "stabilize", 0.0, 0.5)
        .expect("re-range succeeds");
    assert_eq!(bounds(&json), (0.0, 0.5));
    let (min, max, _) = scalar_control(&engine, "Stabilize");
    assert_eq!(
        (min, max),
        (0.0, 50.0),
        "a stored 0.0-0.5 is the 0-50% band the artist reads"
    );

    // The same call with a unit override in force stores the same numbers.
    // If the handler converted, this would land at 0.005.
    set_unit(&mut engine, "brush_settings.stabilize", Some(UnitType::Raw));
    let json = engine
        .brush_graph_set_port_range("brush_settings", "stabilize", 0.0, 0.5)
        .expect("re-range succeeds");
    assert_eq!(
        bounds(&json),
        (0.0, 0.5),
        "the handler must not read its bounds through the entry's unit"
    );
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
    // artist-scrubbable at all.
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

// ── invert: the authored mirror ──────────────────────────────────────

/// The headline feature: an inverted entry reports a mirrored number while
/// the port keeps storing what it always stored, and the bounds it reports
/// do not move (reflecting about `min + max` maps the interval onto itself).
#[test]
fn invert_mirrors_the_brush_bar_value() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    // Hair exposes `multiply_2.a` as "Twirl" over a bipolar -1..1, stored 0.5.
    let (min, max, value) = scalar_control(&engine, "Twirl");
    assert_eq!((min, max, value), (-1.0, 1.0, 0.5));

    let json = set_invert(&mut engine, "multiply_2.a", true);

    let (min, max, value) = scalar_control(&engine, "Twirl");
    assert_eq!(
        value, -0.5,
        "pivot is min+max = 0, so the control reads the stored 0.5 as -0.5"
    );
    assert_eq!(
        (min, max),
        (-1.0, 1.0),
        "the mirror swaps the bounds onto themselves, so the reported range is unchanged"
    );
    assert_eq!(
        stored_value(&json, "multiply_2", "a"),
        0.5,
        "inverting is a display concern: the stored value must not move"
    );
}

/// Anti-double-application: a display number written back through the bar
/// lands mirrored exactly once, and reads back as the number that was sent.
/// A mirror applied twice on either path would surface here.
#[test]
fn inverted_write_round_trips_to_the_stored_value() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");
    set_invert(&mut engine, "multiply_2.a", true);

    // The bar sends the number the artist sees.
    let json = engine
        .brush_set_exposed_port("multiply_2", "a", 0.8)
        .expect("scrub succeeds");

    assert_eq!(
        stored_value(&json, "multiply_2", "a"),
        -0.8,
        "display 0.8 mirrors about 0 to a stored -0.8"
    );
    let (_, _, value) = scalar_control(&engine, "Twirl");
    assert_eq!(
        value, 0.8,
        "and it reads back as sent; a doubled mirror would return -0.8"
    );
}

/// The mirror is an involution, so toggling it consumes no state: the
/// control returns to exactly the number it started at.
#[test]
fn mirror_is_an_involution() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    set_invert(&mut engine, "multiply_2.a", true);
    assert_eq!(scalar_control(&engine, "Twirl").2, -0.5);
    set_invert(&mut engine, "multiply_2.a", false);
    assert_eq!(scalar_control(&engine, "Twirl").2, 0.5);
}

/// The mirror happens in port space, which for every unit that exists today
/// is numerically the same as mirroring in display space (all conversions
/// are pure scales with no offset). This pins that equivalence on a
/// `Percent` port, so a future offset unit fails here rather than silently
/// moving every inverted control.
///
/// The value has to be moved off the pivot first. `brush_settings.stabilize`
/// ships at 0.5 on a 0..1 range whose pivot is 1.0, so its mirror is itself:
/// asserting on the shipped value would pass against an implementation with
/// no mirror at all.
#[test]
fn percent_port_mirrors_identically_in_port_and_display_space() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    // Off the midpoint, in display space: 20% of a Percent port.
    let json = engine
        .brush_set_exposed_port("brush_settings", "stabilize", 20.0)
        .expect("scrub succeeds");
    assert!(
        (stored_value(&json, "brush_settings", "stabilize") - 0.2).abs() < 1e-6,
        "display 20% stores as raw 0.2"
    );

    set_invert(&mut engine, "brush_settings.stabilize", true);

    let (min, max, value) = scalar_control(&engine, "Stabilize");
    assert_eq!((min, max), (0.0, 100.0), "display bounds are unchanged");
    assert!(
        (value - 80.0).abs() < 1e-4,
        "mirroring raw 0.2 about the raw pivot 1.0 gives 0.8, i.e. 80%; got {value}"
    );
}

/// The mirror reflects about the control's *own* bounds, so it equals the
/// complement only when those bounds sum to the full amount. An author who
/// narrows a range and labels the control "Hardness" gets a reflection
/// within the narrowed band, not a 0-100% opposite.
#[test]
fn mirror_reflects_within_the_controls_own_range() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    engine
        .brush_set_exposed_port("brush_settings", "stabilize", 10.0)
        .expect("scrub succeeds");
    engine
        .brush_graph_set_port_range("brush_settings", "stabilize", 0.0, 0.5)
        .expect("re-range succeeds");
    set_invert(&mut engine, "brush_settings.stabilize", true);

    let (min, max, value) = scalar_control(&engine, "Stabilize");
    assert_eq!((min, max), (0.0, 50.0));
    assert!(
        (value - 40.0).abs() < 1e-4,
        "pivot is the narrowed 0+0.5, so raw 0.1 reads as 40%, not the complement 90%; got {value}"
    );
}

/// The guarantee the whole design rests on: `invert` is presentation, so
/// the shader the graph compiles to is byte-identical with it on and off.
///
/// Asserted on the compiled WGSL, not on the serialized graph:
/// `Graph::exposed_ports` is serialized by design, so `invert: true` does
/// appear in the graph JSON.
#[test]
fn inverting_an_entry_does_not_change_what_the_graph_evaluates() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    let wgsl = |engine: &DarklyEngine| {
        darkly::brush::compile_graph(&engine.active_brush_graph())
            .expect("Hair compiles")
            .compiled_brush()
            .expect("Hair has a terminal")
            .stroke_wgsl
            .clone()
    };

    let before = wgsl(&engine);
    set_invert(&mut engine, "multiply_2.a", true);
    assert_eq!(
        before,
        wgsl(&engine),
        "an inverted control must compile to the same shader"
    );
}

/// The override changes what the control reads and nothing else, and it is
/// reversible: clearing it restores the original reading exactly. That is
/// the guarantee that makes trying a unit out safe.
#[test]
fn unit_override_changes_only_what_the_control_shows() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    // `brush_settings.stabilize` is `Percent`, stored 0.5, so it reads 50.
    let (min, max, value) = scalar_control(&engine, "Stabilize");
    assert_eq!((min, max), (0.0, 100.0));
    assert!(
        (value - 50.0).abs() < 1e-4,
        "stored 0.5 reads 50%, got {value}"
    );

    let json = set_unit(&mut engine, "brush_settings.stabilize", Some(UnitType::Raw));
    assert!(
        (stored_value(&json, "brush_settings", "stabilize") - 0.5).abs() < 1e-6,
        "overriding the unit must not touch the stored value"
    );
    let (min, max, value) = scalar_control(&engine, "Stabilize");
    assert_eq!(
        (min, max),
        (0.0, 1.0),
        "the same bounds read in the new unit"
    );
    assert!((value - 0.5).abs() < 1e-6, "raw 0.5 reads 0.5, got {value}");

    set_unit(&mut engine, "brush_settings.stabilize", None);
    let (min, max, value) = scalar_control(&engine, "Stabilize");
    assert_eq!((min, max), (0.0, 100.0));
    assert!(
        (value - 50.0).abs() < 1e-4,
        "clearing the override restores the original reading, got {value}"
    );
}

/// The override applies to writes as well as reads. A one-sided override
/// (converting what the bar shows but not what it writes) would move the
/// stored value every time the artist touched the control.
#[test]
fn unit_override_converts_a_scrub_written_from_the_bar() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");
    set_unit(
        &mut engine,
        "brush_settings.stabilize",
        Some(UnitType::Degrees),
    );

    // 90 degrees in, radians stored, 90 back out.
    let json = engine
        .brush_set_exposed_port("brush_settings", "stabilize", 90.0)
        .expect("scrub succeeds");
    assert!(
        (stored_value(&json, "brush_settings", "stabilize") - std::f64::consts::FRAC_PI_2).abs()
            < 1e-6,
        "a degrees control stores radians"
    );
    let (_, _, value) = scalar_control(&engine, "Stabilize");
    assert!(
        (value - 90.0).abs() < 1e-3,
        "and reads back as 90, got {value}"
    );
}

/// The payload tells the authoring modal what its inherit row falls back
/// to, so the frontend never re-derives the precedence chain. It names the
/// port's own unit whether or not an override is in force.
#[test]
fn inherited_unit_names_the_ports_own_unit() {
    let mut engine = fresh_engine();
    engine.brush_load("Hair").expect("Hair builtin loads");

    let inherited = |engine: &DarklyEngine| {
        let info = engine
            .brush_exposed_ports()
            .into_iter()
            .find(|p| p.key == "brush_settings.stabilize")
            .expect("stabilize is exposed");
        match info.data {
            ExposedValue::Scalar {
                unit_type,
                unit_override,
                inherited_unit,
                ..
            } => (unit_type, unit_override, inherited_unit),
            other => panic!("not a scalar control: {other:?}"),
        }
    };

    assert_eq!(
        inherited(&engine),
        (UnitType::Percent, None, UnitType::Percent),
        "with no override the resolved and inherited units agree"
    );

    set_unit(&mut engine, "brush_settings.stabilize", Some(UnitType::Raw));
    assert_eq!(
        inherited(&engine),
        (UnitType::Raw, Some(UnitType::Raw), UnitType::Percent),
        "an override changes the resolved unit but not what inheriting would mean"
    );
}
