//! Temporary repro tests for layer-panel drag bugs (to be folded into
//! effect_space.rs once diagnosed).

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
        .unwrap_or_else(|| panic!("`{pipeline}` should be addable"))
}

fn tree_json(engine: &DarklyEngine) -> serde_json::Value {
    serde_json::to_value(engine.layer_tree()).expect("layer_tree serializes")
}

fn row_id(row: &serde_json::Value) -> LayerId {
    LayerId::from_ffi(row["id"].as_f64().expect("row carries an id") as u64)
}

/// Root rows, top-first (panel order).
fn root_rows(engine: &DarklyEngine) -> Vec<LayerId> {
    tree_json(engine)["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .map(row_id)
        .collect()
}

fn run_ids(engine: &DarklyEngine) -> Vec<LayerId> {
    let tree = tree_json(engine);
    let count = tree["screenSpaceCount"].as_u64().expect("count") as usize;
    tree["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .take(count)
        .map(row_id)
        .rev()
        .collect()
}

/// Children of a group row, top-first (panel order).
fn group_children(engine: &DarklyEngine, group: LayerId) -> Vec<LayerId> {
    tree_json(engine)["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row_id(row) == group)
        .expect("group row")["children"]
        .as_array()
        .expect("children")
        .iter()
        .map(row_id)
        .collect()
}

/// Bug 1: dragging an effect group from canvas space to a specific slot in the
/// run must land at that slot, not at the bottom of the run.
#[test]
fn dragging_a_group_into_the_run_lands_where_dropped() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let g_eff = effect(&mut engine, "invert");
    let e1 = effect(&mut engine, "grain");
    let e2 = effect(&mut engine, "vhs");
    engine.set_screen_space_boundary(2);
    assert_eq!(run_ids(&engine), vec![e1, e2]);

    let group = engine.group_layers(vec![g_eff]).expect("group the effect");
    assert_eq!(run_ids(&engine), vec![e1, e2], "grouping changed nothing");

    // Panel gesture: drop the group directly above e1's row (below e2) —
    // wire target `after e1`.
    engine
        .move_layers(vec![group], MoveTarget::After(e1))
        .expect("move into the run");

    assert_eq!(
        run_ids(&engine),
        vec![e1, group, e2],
        "the group lands between e1 and e2, where it was dropped"
    );
}

/// Bug 2: dragging an effect layer to a specific slot inside a run group must
/// land at that slot, not at the bottom of the group.
#[test]
fn dragging_an_effect_into_a_run_group_lands_where_dropped() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let e_b = effect(&mut engine, "vhs");
    let group = engine.group_layers(vec![e_a, e_b]).expect("group");
    engine.set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![group]);
    assert_eq!(group_children(&engine, group), vec![e_b, e_a]);

    let e_c = effect(&mut engine, "invert");
    // e_c was appended at root top; with a run present it should be below the
    // divider unless it claims otherwise. Wherever it is, now drag it above
    // e_b's row inside the group (panel top of the group) — wire `after e_b`.
    engine
        .move_layers(vec![e_c], MoveTarget::After(e_b))
        .expect("move into the group");

    assert_eq!(
        group_children(&engine, group),
        vec![e_c, e_b, e_a],
        "e_c lands above e_b, where it was dropped"
    );
    println!("root rows now: {:?}", root_rows(&engine));
}

/// Bug 2 variant: dropping onto the group header ("into") targets the group
/// top.
#[test]
fn dropping_onto_a_run_group_header_lands_on_top() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let e_b = effect(&mut engine, "vhs");
    let group = engine.group_layers(vec![e_a, e_b]).expect("group");
    engine.set_screen_space_boundary(1);

    let e_c = effect(&mut engine, "invert");
    engine
        .move_layers(vec![e_c], MoveTarget::IntoGroupTop(group))
        .expect("move into the group");

    assert_eq!(
        group_children(&engine, group),
        vec![e_c, e_b, e_a],
        "into-top means panel top"
    );
}

/// Bug 1 (suspected mechanism): an EMPTY group dragged into the run must land
/// where dropped, not at the bottom of the run.
#[test]
fn dragging_an_empty_group_into_the_run_lands_where_dropped() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e1 = effect(&mut engine, "grain");
    let e2 = effect(&mut engine, "vhs");
    engine.set_screen_space_boundary(2);
    assert_eq!(run_ids(&engine), vec![e1, e2]);

    let group = engine.add_group(None);
    assert!(!run_ids(&engine).contains(&group), "new group starts below");

    // Panel gesture: drop the empty group above e2's row (top of the run) —
    // wire target `after e2`.
    engine
        .move_layers(vec![group], MoveTarget::After(e2))
        .expect("move into the run");

    assert_eq!(
        run_ids(&engine),
        vec![e1, e2, group],
        "the empty group lands above e2, where it was dropped"
    );
}

/// Bug 2 (suspected mechanism, engine side): what the panel sends when you
/// drop on the lower quarter of an expanded group header is `before group` —
/// a sibling slot BELOW the entire group — even though the indicator line is
/// drawn at the header's bottom edge, which for an expanded group visually
/// points at the top slot INSIDE it. This test documents the engine's
/// (correct) handling of what the panel sends; the mismatch is frontend.
#[test]
fn before_group_is_a_sibling_below_the_whole_group() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let e_b = effect(&mut engine, "vhs");
    let group = engine.group_layers(vec![e_a, e_b]).expect("group");
    engine.set_screen_space_boundary(1);

    let e_c = effect(&mut engine, "invert");
    engine
        .move_layers(vec![e_c], MoveTarget::Before(group))
        .expect("move before the group");

    assert!(
        !group_children(&engine, group).contains(&e_c),
        "before-group lands outside the group"
    );
    // Panel order at root: group above e_c — i.e. e_c renders directly under
    // the group's last child, which reads as \"the bottom of the group\".
    let rows = root_rows(&engine);
    let gi = rows.iter().position(|&r| r == group).unwrap();
    assert_eq!(rows[gi + 1], e_c, "e_c sits directly below the group block");
}

/// Bug 3: unchecking passthrough on a run group currently silently degrades
/// the run (its effects stop rendering).
#[test]
fn unchecking_passthrough_on_a_run_group_keeps_effects_running() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let group = engine.group_layers(vec![e_a]).expect("group");
    engine.set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![group]);
    assert_eq!(engine.test_screen_space_effects(), vec![e_a]);

    engine.set_group_passthrough(group, false);

    assert_eq!(
        run_ids(&engine),
        vec![group],
        "the group stays in the run when passthrough is unchecked"
    );
    assert_eq!(
        engine.test_screen_space_effects(),
        vec![e_a],
        "and its effects keep contributing to the present chain"
    );
}
