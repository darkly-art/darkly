//! Regression tests for removing a **modifier** (mask) by id through the
//! generic layer-removal entry points.
//!
//! A mask is a `Filter` on a host's `filters` list, not a tree node — but it is
//! a selectable row in the layer panel, so the Delete hotkey forwards its id to
//! `remove_layer` just like a layer's. These pin that every generic entry point
//! (`remove_layer`, `remove_layers`, `merge_down`) recognises a modifier id and
//! routes it to the modifier path instead of treating it as a tree node, and
//! that undo puts the mask back on its host rather than in a `children` list.
//!
//! Run with:
//! `cargo test -p darkly --test modifier_removal --features testing -- --test-threads=1`

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// True when `id` is a top-level row of the serializable tree. Note that a
/// *filter* id can never appear here even if the document has wrongly parked it
/// in a children list — `node_to_layer_info` resolves nodes only — so absence is
/// not evidence about filters. The document-level inline tests cover that.
fn has_top_level_row(e: &DarklyEngine, id: LayerId) -> bool {
    let want = id.to_ffi() as f64;
    let tree = serde_json::to_value(e.layer_tree()).expect("layer_tree serializes");
    tree["layers"]
        .as_array()
        .map(|rows| {
            rows.iter().any(|row| {
                row.get("id")
                    .and_then(|v| v.as_f64())
                    .is_some_and(|got| (got - want).abs() < 0.5)
            })
        })
        .unwrap_or(false)
}

/// B1 — `remove_layer` on a mask id must actually detach the mask.
#[test]
fn remove_layer_on_a_mask_id_detaches_the_mask() {
    let mut e = test_engine(32, 32);
    let host = e.add_raster_layer(None);
    let _other = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.host_mask_id(host).expect("mask present");

    e.remove_layer(mask).expect("removing a mask must succeed");

    assert!(
        e.host_mask_id(host).is_none(),
        "the mask must be gone from its host"
    );
    assert_eq!(
        e.layer_tree().layers.len(),
        2,
        "only the two raster layers remain as rows"
    );
}

/// B2 — undo restores the mask onto its host, not into the root's children.
#[test]
fn undoing_a_mask_removal_restores_it_on_its_host() {
    let mut e = test_engine(32, 32);
    let host = e.add_raster_layer(None);
    let _other = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.host_mask_id(host).expect("mask present");

    e.remove_layer(mask).expect("removing a mask must succeed");
    assert!(
        e.host_mask_id(host).is_none(),
        "precondition: the mask must actually have been removed"
    );
    e.undo();

    assert_eq!(
        e.host_mask_id(host),
        Some(mask),
        "undo must reattach the mask to its original host"
    );
    assert_eq!(
        e.layer_tree().layers.len(),
        2,
        "undo must not add a row — the mask belongs to its host, not the root"
    );
}

/// B3 — the batch path must not silently drop modifier ids.
#[test]
fn remove_layers_handles_a_modifier_id_in_the_batch() {
    let mut e = test_engine(32, 32);
    let host = e.add_raster_layer(None);
    let victim = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.host_mask_id(host).expect("mask present");

    let skipped = e
        .remove_layers(vec![mask, victim])
        .expect("batch removal must succeed");

    assert_eq!(skipped, 0);
    assert!(
        e.host_mask_id(host).is_none(),
        "the mask in the batch must be removed, not skipped"
    );
    assert!(!has_top_level_row(&e, victim));
}

/// B6 — a mask and a layer removed together undo as one step.
#[test]
fn a_single_undo_restores_both_a_batched_mask_and_layer() {
    let mut e = test_engine(32, 32);
    let host = e.add_raster_layer(None);
    let victim = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.host_mask_id(host).expect("mask present");

    e.remove_layers(vec![mask, victim])
        .expect("batch removal must succeed");
    assert!(
        e.host_mask_id(host).is_none(),
        "precondition: the batch must actually have removed the mask"
    );
    e.undo();

    assert_eq!(
        e.host_mask_id(host),
        Some(mask),
        "one undo must restore the mask"
    );
    assert!(
        has_top_level_row(&e, victim),
        "the same undo must restore the layer"
    );
}

/// B5 — a mask is not a layer, so deleting the only mask of the only layer
/// must not trip the "cannot delete the last layer" guard.
#[test]
fn removing_the_only_mask_of_the_only_layer_is_allowed() {
    let mut e = test_engine(32, 32);
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.host_mask_id(host).expect("mask present");

    e.remove_layer(mask)
        .expect("a mask is not the last layer — removal must be allowed");

    assert!(e.host_mask_id(host).is_none());
    assert!(has_top_level_row(&e, host), "the host layer must survive");
}

/// B5 (companion) — the guard still fires for the genuine last layer.
#[test]
fn removing_the_last_layer_is_still_refused() {
    let mut e = test_engine(32, 32);
    let only = e.add_raster_layer(None);
    assert_eq!(
        e.remove_layer(only),
        Err("Cannot delete the last layer".to_string())
    );
}

/// B7 — `merge_down` must reject a modifier id before it reaches the
/// sibling-index arithmetic, which reads the host's `children` list using a
/// position taken from its `filters` list.
#[test]
fn merge_down_rejects_a_modifier_id() {
    let mut e = test_engine(32, 32);
    let host = e.add_raster_layer(None);
    let _below = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.host_mask_id(host).expect("mask present");

    assert_eq!(
        e.merge_down(mask),
        Err("Layer not in tree".to_string()),
        "a mask has no sibling below it to merge into"
    );
}
