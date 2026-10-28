//! The hover dab shows the brush's tip at full strength whatever its flow.
//!
//! One hover dab carries one dab's flow, while a stroke stacks many, so a
//! low-flow brush paints visibly and hovers as a near-invisible dab unless
//! the preview is normalized. The preview is stretched so its strongest
//! texel reaches full coverage, rendered at enough precision that a faint
//! dab keeps its falloff through the stretch.
//!
//! Driven through the analytic disc fixture, whose flow per dab is
//! `pressure * multiply.b`; the hover pose has full pressure, so `b` is the
//! dab's flow exactly.

use darkly::brush::portable::PortableBrush;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

/// A soft edge gives the stretch a falloff to preserve.
const SOFTNESS: f32 = 0.3;

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    DarklyEngine::new(GpuContext::new_headless(device, queue), 1024, 768)
}

fn node_id(engine: &DarklyEngine, type_id: &str) -> String {
    engine
        .active_brush_graph()
        .nodes()
        .values()
        .find(|n| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in active graph"))
        .id
        .0
        .clone()
}

/// Install the disc fixture with a soft edge and the given flow, and let
/// any readback the install queued land.
fn install_disc(engine: &mut DarklyEngine, flow: f32) {
    let portable: PortableBrush =
        serde_yaml_ng::from_str(include_str!("fixtures/analytic_disc.yaml"))
            .expect("fixture parses");
    let mut graph = portable
        .into_brush(darkly::brush::registry(), "fixture")
        .expect("fixture builds")
        .metadata
        .graph;
    let id_of = |graph: &darkly::nodegraph::Graph<_>, type_id: &str| {
        graph
            .nodes()
            .values()
            .find(|n| n.type_id == type_id)
            .unwrap_or_else(|| panic!("fixture has a {type_id}"))
            .id
            .clone()
    };
    let mul = id_of(&graph, "multiply");
    let circle = id_of(&graph, "circle");
    graph
        .set_port_default(&mul, "b", flow)
        .expect("multiply factor");
    graph
        .set_port_default(&circle, "softness", SOFTNESS)
        .expect("circle softness");
    engine
        .set_brush_graph(&serde_json::to_string(&graph).unwrap())
        .expect("fixture compiles");
    engine.test_flush_readbacks();
    engine.test_flush_readbacks();
}

fn peak(coverage: &[u8]) -> u8 {
    coverage.iter().copied().max().unwrap_or(0)
}

/// Regression: at 1% flow the hover dab peaked at 3/255. The normalization
/// only counted texels above 12/255 when finding the dab, found none, and
/// applied no boost.
#[test]
fn low_flow_hover_dab_shows_the_tip_at_full_strength() {
    let mut engine = fresh_engine();
    install_disc(&mut engine, 0.01);
    let (shown, _, _) = engine.test_readback_cursor_preview_displayed();
    let p = peak(&shown);
    assert!(
        p >= 250,
        "a 1% flow hover dab should show its tip at full strength; its \
         brightest texel drew {p}/255"
    );
}

/// Regression: an 8-bit preview quantized a 1% flow dab to four levels, so
/// no stretch afterwards could recover its falloff. The displayed dab must
/// be the same at any flow.
#[test]
fn hover_dab_is_flow_invariant() {
    let mut engine = fresh_engine();
    install_disc(&mut engine, 1.0);
    let (full, w, h) = engine.test_readback_cursor_preview_displayed();
    install_disc(&mut engine, 0.01);
    let (faint, w2, h2) = engine.test_readback_cursor_preview_displayed();
    assert_eq!((w, h), (w2, h2), "both hovers render the same mask size");
    let worst = full
        .iter()
        .zip(&faint)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 2,
        "the hover dab at 1% flow should match the one at full flow; the \
         largest per-texel difference was {worst}/255"
    );
}

/// Regression: the normalization was measured once per graph topology, so a
/// scrub that keeps the topology left the hover dab drawn with the gain
/// measured for the old value.
#[test]
fn scrub_that_keeps_topology_restretches_the_hover_dab() {
    let mut engine = fresh_engine();
    install_disc(&mut engine, 0.3);
    let mul = node_id(&engine, "multiply");
    engine
        .brush_set_exposed_port(&mul, "b", 0.1)
        .expect("scrub multiply factor");
    engine.test_flush_readbacks();
    engine.test_flush_readbacks();
    let (shown, _, _) = engine.test_readback_cursor_preview_displayed();
    let p = peak(&shown);
    assert!(
        p >= 250,
        "after scrubbing the flow down, the hover dab should still show its \
         tip at full strength; its brightest texel drew {p}/255"
    );
}

/// A dab already at full strength is drawn as rendered, with no boost that
/// would saturate its falloff.
#[test]
fn full_strength_dab_is_unchanged() {
    let mut engine = fresh_engine();
    install_disc(&mut engine, 1.0);
    let (raw, _, _) = engine.test_readback_cursor_preview_raw();
    let (shown, _, _) = engine.test_readback_cursor_preview_displayed();
    let worst = raw
        .iter()
        .zip(&shown)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "a full-flow hover dab should be drawn as rendered; the largest \
         per-texel difference was {worst}/255"
    );
}
