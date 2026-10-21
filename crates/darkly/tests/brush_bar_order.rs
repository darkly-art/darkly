//! Drag-reordering the brush bar.
//!
//! The bar renders a filtered view of `Graph::exposed_ports`: an exposed
//! input that is driven by a wire stays in the map (its authored meta and
//! its slot survive, so disconnecting brings the control back where it was)
//! but is not shown. A reorder therefore has to land where the artist
//! dropped the row in the *visible* list, whatever hidden entries sit
//! between the rows.
//!
//! The engine's default graph carries one such entry: `paint.build_flow`,
//! exposed by registration and wired from pen pressure, sitting between
//! `paint.wash_flow` and `paint.opacity`.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

const HIDDEN: &str = "paint.build_flow";

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 256, 256)
}

/// The keys the brush bar renders, in order.
fn visible(engine: &DarklyEngine) -> Vec<String> {
    engine
        .brush_exposed_ports()
        .into_iter()
        .map(|p| p.key)
        .collect()
}

/// Perform the reorder the brush bar sends when `key` is dropped at
/// position `to` of the visible list: the dragged key, anchored on the
/// visible row that now sits directly below it.
fn drag(engine: &mut DarklyEngine, key: &str, to: usize) {
    let mut order = visible(engine);
    let from = order
        .iter()
        .position(|k| k == key)
        .expect("dragged key is visible");
    let moved = order.remove(from);
    order.insert(to, moved);
    let before = order.get(to + 1).cloned();
    engine
        .brush_graph_reorder_exposed_port(key, before)
        .expect("reorder");
}

/// Pin the premise: the hidden entry is in the map but not on the bar.
fn assert_hidden_entry(engine: &DarklyEngine) {
    let graph = engine.active_brush_graph();
    assert!(
        graph.exposed_ports.contains_key(HIDDEN),
        "default graph should expose {HIDDEN}"
    );
    assert!(
        !visible(engine).iter().any(|k| k == HIDDEN),
        "{HIDDEN} is wired, so the bar should not show it"
    );
}

#[test]
fn drag_down_one_past_a_hidden_entry_moves_the_visible_row() {
    let mut engine = fresh_engine();
    assert_hidden_entry(&engine);
    assert_eq!(
        visible(&engine),
        [
            "brush_settings.size",
            "paint.wash_flow",
            "paint.opacity",
            "paint.buildup"
        ]
    );

    drag(&mut engine, "paint.wash_flow", 2);

    assert_eq!(
        visible(&engine),
        [
            "brush_settings.size",
            "paint.opacity",
            "paint.wash_flow",
            "paint.buildup"
        ]
    );
}

#[test]
fn drag_up_one_past_a_hidden_entry_moves_exactly_one_row() {
    let mut engine = fresh_engine();
    assert_hidden_entry(&engine);
    engine
        .brush_graph_expose_port("circle", "softness")
        .expect("expose circle.softness");
    assert_eq!(
        visible(&engine),
        [
            "brush_settings.size",
            "paint.wash_flow",
            "paint.opacity",
            "paint.buildup",
            "circle.softness",
        ]
    );

    drag(&mut engine, "circle.softness", 3);

    assert_eq!(
        visible(&engine),
        [
            "brush_settings.size",
            "paint.wash_flow",
            "paint.opacity",
            "circle.softness",
            "paint.buildup",
        ]
    );
}
