//! Moving nodes across the viewport divider.
//!
//! Regression coverage for the two drag bugs born of the boundary having no
//! index in the tree (`docs/plans/divider-as-a-node.md`): a group dragged from
//! screen space to just below the viewport threshold either landed at the
//! bottom of screen space (reference-side inheritance) or refused with
//! "Cannot move a layer into itself" (the boundary gap resolving into the
//! dragged subtree). With the divider as a node, the gesture's encoding is
//! `Before(divider)` — a target that did not exist under the count design —
//! and the count-era encodings are asserted alongside as the failing baselines
//! they were.
//!
//! Run with: `cargo test -p darkly --test divider_moves --features testing -- --test-threads=1`

use darkly::document::MoveTarget;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn effect(engine: &mut DarklyEngine, pipeline: &str) -> LayerId {
    let defaults: Vec<_> = engine
        .filter_param_defs(pipeline)
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect();
    engine
        .add_filter_layer(pipeline, defaults, None)
        .unwrap_or_else(|| panic!("`{pipeline}` should be addable as an effect layer"))
}

fn tree_json(engine: &DarklyEngine) -> serde_json::Value {
    serde_json::to_value(engine.layer_tree()).expect("layer_tree serializes")
}

fn row_id(row: &serde_json::Value) -> LayerId {
    LayerId::from_ffi(row["id"].as_f64().expect("row carries an id") as u64)
}

/// Run members, bottom-to-top — the rows above the divider row, reversed.
fn run_ids(engine: &DarklyEngine) -> Vec<LayerId> {
    let tree = tree_json(engine);
    let mut ids: Vec<LayerId> = tree["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .take_while(|row| row["type"] != "divider")
        .map(row_id)
        .collect();
    ids.reverse();
    ids
}

fn divider_row_id(engine: &DarklyEngine) -> LayerId {
    let tree = tree_json(engine);
    let row = tree["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["type"] == "divider")
        .expect("the tree always carries the divider row")
        .clone();
    row_id(&row)
}

fn in_run(engine: &DarklyEngine, id: LayerId) -> bool {
    run_ids(engine).contains(&id)
}

/// The user's report, step 2: a veil in a subgroup of a viewport group
/// ("VHS in Viewport Effects → Group 2"). Dragging Group 2 to just below the
/// viewport threshold must land it in canvas space — not at the bottom of
/// screen space.
///
/// The gesture's encoding is `Before(divider)`: the divider is a row, so the
/// gap below it exists and names the divider. The count design's only
/// encoding, `Before(VE)` (the escape-outward ancestor reference), inherited
/// VE's side of the boundary and kept Group 2 in the run.
#[test]
fn dragging_a_nested_group_below_the_threshold_lands_in_canvas_space() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let anchor_effect = effect(&mut engine, "invert");
    let vhs = effect(&mut engine, "vhs");
    let g2 = engine.group_layers(vec![vhs]).expect("group the veil");
    let ve = engine
        .group_layers(vec![anchor_effect, g2])
        .expect("viewport effects group");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![ve], "the outer group is the run");

    let divider = divider_row_id(&engine);
    engine
        .move_layers(vec![g2], MoveTarget::Before(divider))
        .expect("dropping just below the threshold is a legal move");
    assert!(
        !in_run(&engine, g2),
        "Group 2 must land in canvas space, not the bottom of screen space"
    );
    assert_eq!(
        run_ids(&engine),
        vec![ve],
        "the run keeps only the viewport group"
    );

    // The count-era baseline, still expressible: `Before(VE)` is now an
    // ordinary verbatim move that means "directly below VE", which is *above*
    // the divider — a screen-space statement, honored as one.
    engine
        .move_layers(vec![g2], MoveTarget::Before(ve))
        .expect("a screen-space drop next to VE is legal for an effect group");
    assert!(
        in_run(&engine, g2),
        "Before(VE) is a screen-space slot, stated and honored verbatim"
    );
}

/// The user's report, step 3: once the group sits at the bottom of screen
/// space, dragging it below the threshold again must succeed — under the count
/// design the gesture's only encoding resolved to the group's own descendant
/// and refused with "Cannot move a layer into itself".
#[test]
fn dragging_the_bottom_run_group_below_the_threshold_is_not_self_referential() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let vhs = effect(&mut engine, "vhs");
    let g2 = engine.group_layers(vec![vhs]).expect("group the veil");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![g2], "the group alone is the run");

    // The count-era encoding is still self-referential — that has not changed
    // and never will (a node cannot be its own sibling anchor)…
    let err = engine
        .move_layers(vec![g2], MoveTarget::Before(vhs))
        .expect_err("a target inside the dragged subtree is self-referential");
    assert!(err.contains("into itself"), "unchanged refusal: {err}");

    // …but the gesture no longer *needs* that encoding: the gap below the
    // divider is a real slot naming the divider row.
    let divider = divider_row_id(&engine);
    engine
        .move_layers(vec![g2], MoveTarget::Before(divider))
        .expect("dropping just below the threshold is a legal move");
    assert!(
        !in_run(&engine, g2),
        "the group must land in canvas space with its veil intact"
    );
    assert!(run_ids(&engine).is_empty(), "the run is empty");
}

/// Moving the divider itself is an ordinary layer move with ordinary undo:
/// drag it below an effect and the effect is viewport-only; undo and it is
/// back in the image.
#[test]
fn moving_the_divider_is_an_ordinary_undoable_move() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    let divider = divider_row_id(&engine);

    engine
        .move_layer(divider, MoveTarget::Before(e))
        .expect("dragging the divider below an effect");
    assert_eq!(run_ids(&engine), vec![e]);

    engine.undo();
    assert!(run_ids(&engine).is_empty(), "undo restores the boundary");
    engine.redo();
    assert_eq!(run_ids(&engine), vec![e], "redo re-applies it");
}

/// The divider refuses the operations that would break its invariant: delete,
/// duplicate, grouping (skipped, the rest still group), and nesting.
#[test]
fn the_divider_refuses_structural_operations() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    let g = engine.group_layers(vec![e]).expect("group");
    let divider = divider_row_id(&engine);

    let err = engine
        .remove_layer(divider)
        .expect_err("the divider cannot be deleted");
    assert!(err.contains("cannot be deleted"), "{err}");

    assert!(
        engine.duplicate_node(divider).is_none(),
        "the divider cannot be duplicated"
    );

    let err = engine
        .move_layer(divider, MoveTarget::IntoGroupTop(g))
        .expect_err("the divider cannot enter a group");
    assert!(err.contains("root"), "{err}");

    // Grouping a selection that includes the divider groups everything else.
    let g2 = engine
        .group_layers(vec![divider, g])
        .expect("grouping skips the divider");
    assert_eq!(
        divider_row_id(&engine),
        divider,
        "the divider survived, un-grouped"
    );
    assert!(
        engine.layer_tree().layers.len() > 1,
        "the group and the divider are both root rows"
    );
    let _ = g2;
}
