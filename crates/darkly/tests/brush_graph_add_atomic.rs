//! Regression: a failed `brush_graph_add_node` must not corrupt the live
//! graph or surface a misleading error.
//!
//! Two compounded bugs once let "add clamp" silently warp the Paint node
//! in the brush builder UI:
//!   1. The Rust mutator inserted the new node *before* validating, so a
//!      compile failure left an orphan node in the active graph.
//!   2. The WGSL compile error was laundered into `GraphError::CycleDetected`
//!      with the real message dropped into stderr.
//!
//! Both surfaced through the bare-default trait `compile_wgsl`, which
//! returns `Err("node has no WGSL implementation")` — the `clamp` node has
//! no WGSL impl, so adding it to the default (paint-terminal) brush graph
//! exercises both failure paths. This test pins both fixes in place.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 1024, 768)
}

#[test]
fn adding_a_non_compilable_node_returns_error_and_leaves_graph_unchanged() {
    let mut engine = fresh_engine();

    // The default brush graph terminates in `paint`, so adding any node
    // without a `compile_wgsl` impl exercises the WGSL compile path.
    // Snapshot the graph before, then try the add.
    let before = engine.active_brush_graph();

    let result = engine.brush_graph_add_node("clamp");

    // The add must fail — clamp has no WGSL impl.
    let err = result.expect_err("adding a non-compilable node must return Err");

    // The error must mention WGSL, not a misleading "cycle detected".
    assert!(
        err.to_lowercase().contains("wgsl"),
        "error should mention WGSL, got: {err:?}",
    );
    assert!(
        !err.to_lowercase().contains("cycle"),
        "error must not be laundered through CycleDetected, got: {err:?}",
    );

    // The graph must be unchanged — no orphan clamp node committed.
    let after = engine.active_brush_graph();
    assert_eq!(
        after.nodes.len(),
        before.nodes.len(),
        "failed add must not insert a node — before had {} nodes, after has {}",
        before.nodes.len(),
        after.nodes.len(),
    );
    assert!(
        after.nodes.values().all(|n| n.type_id != "clamp"),
        "failed add must not leave a clamp node in the committed graph",
    );
}

#[test]
fn adding_a_compilable_node_still_works() {
    // Sanity check that the atomic-commit path didn't break the happy
    // case. `multiply` has a `compile_wgsl` impl and is used by shipping
    // brushes, so it should add cleanly to the default graph.
    let mut engine = fresh_engine();
    let before = engine.active_brush_graph();
    let result = engine.brush_graph_add_node("multiply");
    assert!(
        result.is_ok(),
        "adding multiply (which has compile_wgsl) must succeed: {:?}",
        result.err(),
    );
    let after = engine.active_brush_graph();
    assert_eq!(
        after.nodes.len(),
        before.nodes.len() + 1,
        "successful add must insert exactly one node",
    );
}
