//! Regression test for the generic in-node preview thumbnail.
//!
//! Before the WGSL node-system overhaul, the brush builder previewed *any*
//! node with a renderable output via a GPU subgraph render. The overhaul left
//! a stub that special-cased `noise` with a CPU renderer and returned an empty
//! `Vec` for every other node — so `shape`, `image`, and the rest lost their
//! preview.
//!
//! This drives the engine handler exactly as the frontend does: add a node,
//! call `brush_node_preview`, pump the async readback loop to completion, and
//! assert the cache filled with real PNG bytes. Against the stubbed handler
//! this fails — a non-`noise` node returns empty regardless of pumping.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn new_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 256, 256);
    // Settle the constructor's initial-graph readback.
    engine.test_flush_readbacks();
    engine
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A `shape` node (renderable `Scalar` mask output, no texture dependency)
/// must produce a non-empty preview through the async engine path. On the
/// stubbed handler this returns empty because `shape` is not `noise`.
#[test]
fn shape_node_produces_non_empty_preview() {
    let mut engine = new_engine();

    // Add a shape node to the active brush graph and take the id the graph
    // assigned it.
    let (_json, node_id) = engine
        .brush_graph_add_node("shape")
        .expect("shape node added to active graph");

    // First call kicks off the async GPU render + readback; it returns empty
    // while the render is pending (by design — no blocking readback).
    let pending = engine.brush_node_preview(&node_id);
    assert!(
        pending.is_empty(),
        "first call is async — no cached bytes yet"
    );

    // Pump the readback loop to completion, filling the per-node cache.
    engine.test_flush_readbacks();
    engine.test_flush_readbacks();

    let bytes = engine.brush_node_preview(&node_id);
    assert!(
        !bytes.is_empty(),
        "after pumping the readback, a renderable node must return preview bytes \
         — empty means the handler still special-cases noise and drops shape",
    );
    assert_eq!(&bytes[..8], &PNG_SIGNATURE, "preview must be a valid PNG",);
}
