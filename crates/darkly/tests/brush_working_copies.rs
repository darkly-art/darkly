//! Per-brush working copies: a painter's unsaved tweaks to a library brush
//! survive switching to another brush and back.
//!
//! The library is process-global, so each test resets it first; otherwise one
//! test's saved brushes leak into the next within this binary.

use darkly::brush::input_value::InputValue;
use darkly::brush::library;
use darkly::engine::{DarklyEngine, ExposedValue};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::NodeId;

fn fresh_engine() -> DarklyEngine {
    library::reset_for_test();
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 1024, 768)
}

/// The active graph's `brush_settings.size` entry as (value, default), both
/// display-space, read the way the brush bar reads it.
fn size(engine: &DarklyEngine) -> (f32, f32) {
    let port = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.node_id == "brush_settings" && p.port_name == "size")
        .expect("the brush exposes brush_settings.size");
    match port.data {
        ExposedValue::Scalar { value, default, .. } => (value, default),
        _ => panic!("size is a scalar"),
    }
}

/// Scrub the active brush's size from the brush bar.
fn scrub_size(engine: &mut DarklyEngine, display: f32) {
    engine
        .brush_set_exposed_port("brush_settings", "size", display)
        .expect("size scrub");
}

#[test]
fn tweak_survives_switching_away_and_back() {
    let mut engine = fresh_engine();
    engine.brush_load("Ink").unwrap();
    let (ink_saved, _) = size(&engine);
    engine.brush_load("Airbrush").unwrap();
    let (air_saved, _) = size(&engine);
    assert_ne!(air_saved, 250.0, "the fixture scrub must be a real change");

    scrub_size(&mut engine, 250.0);
    engine.brush_load("Ink").unwrap();
    assert_eq!(
        size(&engine).0,
        ink_saved,
        "the tweak does not leak into another brush"
    );

    engine.brush_load("Airbrush").unwrap();
    assert_eq!(
        size(&engine).0,
        250.0,
        "the tweak survives switching away and back"
    );
}

/// A library brush's id, looked up by its display name.
fn id_of(engine: &DarklyEngine, name: &str) -> String {
    engine
        .brush_list()
        .into_iter()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("brush '{name}' is in the library"))
        .id
}

/// `brush_settings.size` on the active graph, in port space, exactly as stored.
fn size_port_value(engine: &DarklyEngine) -> f32 {
    engine
        .active_brush_graph()
        .nodes()
        .get(&NodeId("brush_settings".into()))
        .and_then(|n| n.ports.iter().find(|p| p.name == "size"))
        .expect("the brush has brush_settings.size")
        .value
        .as_f32()
}

/// (id, modified) of the loaded brush, or `None` when the graph is nobody's.
fn loaded(engine: &DarklyEngine) -> Option<(String, bool)> {
    engine.brush_loaded().map(|b| (b.id, b.modified))
}

#[test]
fn reset_default_is_the_saved_value_under_a_working_copy() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    let (saved, _) = size(&engine);
    scrub_size(&mut engine, 250.0);
    engine.brush_load("Ink").unwrap();
    engine.brush_load("Airbrush").unwrap();

    let (value, default) = size(&engine);
    assert_eq!(value, 250.0);
    assert_eq!(
        default, saved,
        "double-click reset returns to the saved brush, not the tweak"
    );
}

#[test]
fn loading_the_loaded_brush_keeps_the_tweak_and_moves_no_counter() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    scrub_size(&mut engine, 250.0);
    let topo = engine.brush_topology_version();
    let version = engine.brush_graph_version();

    engine.brush_load("Airbrush").unwrap();
    assert_eq!(size(&engine).0, 250.0);
    assert_eq!(engine.brush_topology_version(), topo);
    assert_eq!(engine.brush_graph_version(), version);
}

#[test]
fn save_clears_the_working_copy() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    let (air_saved, _) = size(&engine);
    scrub_size(&mut engine, 250.0);
    engine.brush_save("mine", "Mine").unwrap();
    assert_eq!(loaded(&engine), Some(("mine".into(), false)));

    engine.brush_load("Ink").unwrap();
    engine.brush_load("Mine").unwrap();
    assert_eq!(
        size(&engine),
        (250.0, 250.0),
        "saved: the tweak is the brush now"
    );

    engine.brush_load("Airbrush").unwrap();
    assert_eq!(
        size(&engine).0,
        air_saved,
        "the tweak moved to the new brush"
    );
    assert_eq!(loaded(&engine), Some((id_of(&engine, "Airbrush"), false)));
}

#[test]
fn save_over_own_brush_clears_it_too() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    engine.brush_save("mine", "Mine").unwrap();
    scrub_size(&mut engine, 250.0);
    assert_eq!(loaded(&engine), Some(("mine".into(), true)));
    engine.brush_save("mine", "Mine").unwrap();

    engine.brush_load("Ink").unwrap();
    engine.brush_load("Mine").unwrap();
    assert_eq!(size(&engine).0, 250.0);
    assert_eq!(loaded(&engine), Some(("mine".into(), false)));
}

#[test]
fn reset_discards_the_working_copy() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    let (saved, _) = size(&engine);
    scrub_size(&mut engine, 250.0);
    engine.reset_brush_graph();
    assert_eq!(loaded(&engine), None);

    engine.brush_load("Ink").unwrap();
    engine.brush_load("Airbrush").unwrap();
    assert_eq!(size(&engine).0, saved);
}

#[test]
fn yaml_import_stashes_the_outgoing_brush() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    scrub_size(&mut engine, 250.0);
    let yaml = engine.brush_export_yaml(&id_of(&engine, "Ink")).unwrap();
    engine.set_brush_graph_yaml(&yaml).unwrap();
    assert_eq!(loaded(&engine), None, "an imported graph is nobody's");

    engine.brush_load("Airbrush").unwrap();
    assert_eq!(size(&engine).0, 250.0);
}

#[test]
fn delete_forgets_the_loaded_brush() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    engine.brush_save("mine", "Mine").unwrap();
    scrub_size(&mut engine, 250.0);
    engine.brush_delete("mine").unwrap();
    assert_eq!(loaded(&engine), None, "a deleted brush is no longer loaded");
    assert_eq!(size(&engine).0, 250.0, "its graph stays live, as nobody's");

    // Leaving a graph that belongs to no brush stashes nothing and fails nothing.
    engine.brush_load("Ink").unwrap();
}

#[test]
fn brush_loaded_reports_name_and_modified() {
    let mut engine = fresh_engine();
    assert_eq!(
        loaded(&engine),
        None,
        "a fresh engine runs the built-in graph"
    );

    engine.brush_load("Airbrush").unwrap();
    let loaded_brush = engine.brush_loaded().unwrap();
    assert_eq!(loaded_brush.name, "Airbrush");
    assert!(!loaded_brush.modified);

    let saved = size_port_value(&engine);
    scrub_size(&mut engine, 250.0);
    assert!(engine.brush_loaded().unwrap().modified);
    // Back through the exact port-space value: the display-to-port percent
    // round trip is not guaranteed to land on the same bits.
    engine
        .brush_graph_set_input("brush_settings", "size", InputValue::Scalar(saved))
        .unwrap();
    assert!(
        !engine.brush_loaded().unwrap().modified,
        "tweaking back is clean"
    );

    engine.brush_save("mine", "Mine").unwrap();
    engine.brush_rename("mine", "Renamed").unwrap();
    assert_eq!(engine.brush_loaded().unwrap().name, "Renamed");
}

#[test]
fn hydration_sequence_leaves_no_copies() {
    let mut engine = fresh_engine();
    let air = engine
        .brush_export_yaml(&id_of(&engine, "Airbrush"))
        .unwrap();
    let ink = engine.brush_export_yaml(&id_of(&engine, "Ink")).unwrap();
    engine.set_brush_graph_yaml(&air).unwrap();
    engine.brush_save("x", "X").unwrap();
    engine.set_brush_graph_yaml(&ink).unwrap();
    engine.brush_save("y", "Y").unwrap();

    engine.brush_load("X").unwrap();
    assert_eq!(loaded(&engine), Some(("x".into(), false)));
    engine.brush_load("Airbrush").unwrap();
    let air_size = size(&engine).0;
    engine.brush_load("X").unwrap();
    assert_eq!(size(&engine).0, air_size);
}

#[test]
fn revert_restores_the_saved_graph_and_clears_modified() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    let (saved, _) = size(&engine);
    scrub_size(&mut engine, 250.0);
    engine.brush_revert().unwrap();
    assert_eq!(size(&engine).0, saved);
    assert_eq!(loaded(&engine), Some((id_of(&engine, "Airbrush"), false)));

    engine.brush_load("Ink").unwrap();
    engine.brush_load("Airbrush").unwrap();
    assert_eq!(
        size(&engine).0,
        saved,
        "a reverted brush has no working copy left"
    );

    engine.reset_brush_graph();
    assert!(
        engine.brush_revert().is_err(),
        "nothing is loaded to revert to"
    );
}

#[test]
fn a_bad_import_leaves_the_loaded_brush_untouched() {
    let mut engine = fresh_engine();
    engine.brush_load("Airbrush").unwrap();
    scrub_size(&mut engine, 250.0);
    assert!(engine.set_brush_graph_yaml("not: a graph").is_err());
    assert_eq!(loaded(&engine), Some((id_of(&engine, "Airbrush"), true)));
    assert_eq!(size(&engine).0, 250.0);
}
