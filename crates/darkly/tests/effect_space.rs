//! The screen-space boundary — the divider that splits the root's children
//! into what the document exports and what only the viewport shows.
//!
//! Everything here is about *space*, not about effects: which side of the line
//! a node is on, who is allowed to be above it, what survives a save, and what
//! undo puts back. The effects themselves are covered by `filters.rs`.
//!
//! Run with: `cargo test -p darkly --test effect_space --features testing -- --test-threads=1`

use darkly::document::MoveTarget;
use darkly::engine::types::StrokeOp;
use darkly::engine::{DarklyEngine, SavePurpose};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Flood-fill a layer with straight opaque `(r, g, b, 255)`, so the composite
/// reads back as the layer colour and `invert` is unambiguous.
fn fill_layer(engine: &mut DarklyEngine, layer_id: LayerId, r: u8, g: u8, b: u8) {
    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r,
        g,
        b,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    settle(engine);
}

/// Let async pixel work land before a readback. A flood fill is a GPU write
/// whose result is not observable until the readbacks drain and a frame runs.
fn settle(engine: &mut DarklyEngine) {
    engine.test_flush_readbacks();
    engine.render(0.0);
}

/// RGBA quad at `(x, y)` in a `stride`-wide buffer.
fn px(buf: &[u8], stride: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * stride + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
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

/// Read the boundary through the layer-tree response — the same view the panel
/// consumes, so these assertions fail if the engine and the UI ever disagree
/// about where the line is.
fn tree_json(engine: &DarklyEngine) -> serde_json::Value {
    serde_json::to_value(engine.layer_tree()).expect("layer_tree serializes")
}

fn row_id(row: &serde_json::Value) -> LayerId {
    LayerId::from_ffi(row["id"].as_f64().expect("row carries an id") as u64)
}

/// Run members, bottom-to-top. The response is top-first and the run is its
/// prefix, so this reverses.
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

fn stored_count(engine: &DarklyEngine) -> usize {
    tree_json(engine)["screenSpaceCount"]
        .as_u64()
        .expect("count") as usize
}

fn in_run(engine: &DarklyEngine, id: LayerId) -> bool {
    run_ids(engine).contains(&id)
}

fn eligible(engine: &DarklyEngine, id: LayerId) -> bool {
    tree_json(engine)["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row_id(row) == id)
        .and_then(|row| row["screenSpaceEligible"].as_bool())
        .unwrap_or(false)
}

/// The id of a row's mask modifier, read out of the same serialized tree.
fn mask_id(engine: &DarklyEngine, host: LayerId) -> LayerId {
    let tree = tree_json(engine);
    let row = tree["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row_id(row) == host)
        .expect("host row");
    row["modifiers"]
        .as_array()
        .and_then(|mods| mods.first())
        .map(row_id)
        .expect("host carries a mask")
}

fn root_row_count(engine: &DarklyEngine) -> usize {
    tree_json(engine)["layers"].as_array().expect("rows").len()
}

// ---------------------------------------------------------------------------
// What the boundary is
// ---------------------------------------------------------------------------

/// The stored count — not tree position, not layer kind — is what decides which
/// space a node renders in.
#[test]
fn boundary_partitions_the_root_into_two_spaces() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let a = effect(&mut engine, "invert");
    let b = effect(&mut engine, "grain");

    engine.set_screen_space_boundary(2);
    assert_eq!(
        run_ids(&engine),
        vec![a, b],
        "both effects are viewport-only"
    );
    assert!(!in_run(&engine, raster));

    engine.set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![b], "only the topmost stays above");

    engine.set_screen_space_boundary(0);
    assert!(run_ids(&engine).is_empty(), "the run empties");
    assert_eq!(
        root_row_count(&engine),
        3,
        "and everything is canvas-space again"
    );
}

/// The one property a user must never be surprised by: what is above the line
/// is not in the image.
#[test]
fn screen_space_effect_is_absent_from_the_composite() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);
    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);
    let baseline = px(&engine.test_readback_canvas(), cw, 8, 8);
    assert_eq!(baseline, [255, 0, 0, 255], "the fill landed");

    let _inv = effect(&mut engine, "invert");
    settle(&mut engine);
    assert_eq!(
        px(&engine.test_readback_canvas(), cw, 8, 8),
        [0, 255, 255, 255],
        "a canvas-space invert turns the red below it cyan"
    );

    engine.set_screen_space_boundary(1);
    settle(&mut engine);
    assert_eq!(
        px(&engine.test_readback_canvas(), cw, 8, 8),
        baseline,
        "moving the same effect above the line restores the composite exactly"
    );
}

/// …and it *is* visible on screen. Same effect, same document, both sides of
/// the divider: the composite readback cannot tell them apart, and the surface
/// readback must.
#[test]
fn screen_space_effect_is_visible_only_after_the_present_pass() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);
    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);

    let _inv = effect(&mut engine, "invert");
    engine.set_screen_space_boundary(1);
    settle(&mut engine);

    // The surface is sRGB-encoded, so this asserts the *ordering* of channels
    // rather than exact bytes: red became cyan if red is now the small one.
    let center = px(&engine.test_readback_screen_run(cw, ch), cw, 8, 8);
    assert!(
        center[0] < 64 && center[1] > 190 && center[2] > 190,
        "a viewport-only invert must show on the surface: red → cyan, got {center:?}"
    );

    // And the same document's composite is untouched, which is the pair of
    // facts that makes "viewport only" mean something.
    assert_eq!(
        px(&engine.test_readback_canvas(), cw, 8, 8),
        [255, 0, 0, 255],
        "the image itself is still red"
    );
}

// ---------------------------------------------------------------------------
// The invariant
// ---------------------------------------------------------------------------

/// The most important test here. Every structural door funnels through
/// `Document::link`, so none of them can put a raster above the boundary — and
/// a new insertion path that *would* need this test edited is exactly the
/// coverage that matters.
#[test]
fn a_raster_can_never_be_placed_above_the_boundary() {
    let mut engine = test_engine(16, 16);
    let base = engine.add_raster_layer(None);
    let e1 = effect(&mut engine, "invert");
    let e2 = effect(&mut engine, "grain");
    let e3 = effect(&mut engine, "vhs");
    engine.set_screen_space_boundary(3);
    let expected = vec![e1, e2, e3];
    assert_eq!(run_ids(&engine), expected);

    // Anchored nowhere — resolves to the top of the root, which is inside the
    // run.
    let added = engine.add_raster_layer(None);
    assert!(!in_run(&engine, added));
    assert_eq!(run_ids(&engine), expected, "add_raster_layer(None)");

    // Anchored *on* the topmost run member, which asks for a slot above it.
    let anchored = engine.add_raster_layer(Some(e3));
    assert!(!in_run(&engine, anchored));
    assert_eq!(run_ids(&engine), expected, "add_raster_layer(Some(top))");

    // A duplicate lands beside its source; a raster's source is below the line,
    // but a duplicate of a run member would be asking for a slot inside it.
    engine.duplicate_node(base).expect("duplicate");
    assert_eq!(run_ids(&engine), expected, "duplicate of a canvas raster");

    // Grouping wraps its sources in a fresh group — a kind that is only
    // eligible when everything inside it is.
    engine.group_layers(vec![base, added]).expect("group");
    assert_eq!(run_ids(&engine), expected, "group_layers");

    // An explicit move that targets the top of the stack.
    engine
        .move_layers(vec![anchored], MoveTarget::After(e3))
        .expect("move above the run");
    assert!(
        !in_run(&engine, anchored),
        "an explicit move above the run must still clamp"
    );
    assert_eq!(run_ids(&engine), expected, "move_layers above the run");
}

/// The structural clauses of the eligibility predicate, and the read clamp that
/// catches the changes insertion cannot see.
#[test]
fn masked_or_isolated_nodes_cannot_be_above_the_boundary() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");

    // A raster is never eligible, so the boundary clamps to the one effect.
    engine.set_screen_space_boundary(2);
    assert_eq!(run_ids(&engine), vec![e], "clamped past the raster");
    assert!(!eligible(&engine, raster));

    // A mask on a run member drops it to canvas space without discarding the
    // user's stated intent…
    engine.add_mask(e);
    assert!(
        run_ids(&engine).is_empty(),
        "a masked node cannot be realized after the view transform"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "the run reads empty while the host is disqualified"
    );

    // …and removing it restores the run from that same stored intent.
    engine.remove_mask(e);
    assert_eq!(run_ids(&engine), vec![e], "the run comes back");
}

/// A group crosses the line whole, and only when everything inside it can.
#[test]
fn a_group_is_eligible_exactly_when_its_contents_are() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    let group = engine.group_layers(vec![e]).expect("group of one effect");

    engine.set_screen_space_boundary(1);
    assert_eq!(
        run_ids(&engine),
        vec![group],
        "a passthrough group of effects is eligible"
    );

    // Isolating it gives it a canvas-space accumulator, which has no
    // screen-space counterpart.
    engine.set_group_passthrough(group, false);
    assert!(
        run_ids(&engine).is_empty(),
        "an isolated group cannot be above the line"
    );

    engine.set_group_passthrough(group, true);
    engine
        .move_layers(vec![raster], MoveTarget::IntoGroupTop(group))
        .expect("move into the group");
    assert!(
        run_ids(&engine).is_empty(),
        "a group holding a raster is not eligible, however passthrough it is"
    );
}

/// Eligibility is structural. Toggling an eye must never change what a document
/// exports.
#[test]
fn visibility_never_changes_which_space_a_node_is_in() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let canvas_effect = effect(&mut engine, "invert");
    engine.add_mask(canvas_effect);
    let run_effect = effect(&mut engine, "grain");
    engine.set_screen_space_boundary(1);
    let before = run_ids(&engine);
    assert_eq!(before, vec![run_effect]);

    for id in [raster, canvas_effect, run_effect] {
        engine.set_layer_visible(id, false);
        assert_eq!(
            run_ids(&engine),
            before,
            "hiding a node must not move the boundary"
        );
        assert_eq!(stored_count(&engine), 1);
        engine.set_layer_visible(id, true);
    }

    let mask = mask_id(&engine, canvas_effect);
    engine.set_layer_visible(mask, false);
    assert_eq!(
        run_ids(&engine),
        before,
        "hiding a mask must not make its host eligible"
    );
}

// ---------------------------------------------------------------------------
// Persistence and undo
// ---------------------------------------------------------------------------

/// One drag, one undo step. A per-layer flag would have needed one per layer.
#[test]
fn boundary_move_is_one_undo_step() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let a = effect(&mut engine, "invert");
    let b = effect(&mut engine, "grain");

    engine.set_screen_space_boundary(2);
    assert_eq!(run_ids(&engine), vec![a, b]);

    engine.undo();
    assert!(run_ids(&engine).is_empty(), "one undo returns the divider");

    engine.redo();
    assert_eq!(run_ids(&engine), vec![a, b], "and redo puts it back");
}

/// The pair of cases a bare index-plus-count cannot both satisfy, and the
/// reason undo carries the side rather than deriving it.
///
/// `E_x` and `E_a` occupy the *same index* at the moment they are reinserted —
/// the lowest slot the run could start at — so position alone cannot say which
/// of them belongs in it.
#[test]
fn undo_restores_run_membership_exactly() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let ex = effect(&mut engine, "invert");
    let ea = effect(&mut engine, "grain");
    let eb = effect(&mut engine, "vhs");
    engine.set_screen_space_boundary(2);
    assert_eq!(run_ids(&engine), vec![ea, eb]);

    // Deleting the lowest run member and undoing must bring it back *into* the
    // run, and must not sweep the canvas-space effect below it in with it.
    engine.remove_layer(ea).expect("remove");
    assert_eq!(run_ids(&engine), vec![eb]);
    engine.undo();
    assert_eq!(
        run_ids(&engine),
        vec![ea, eb],
        "the deleted run member rejoins the run"
    );
    assert!(!in_run(&engine, ex));

    // The mirror case: deleting the topmost canvas-space node and undoing must
    // leave it below the line, at the very same index.
    engine.remove_layer(ex).expect("remove");
    engine.undo();
    assert!(
        !in_run(&engine, ex),
        "a canvas-space node adjacent to the divider must not cross it on undo"
    );
    assert_eq!(run_ids(&engine), vec![ea, eb]);
}

/// The boundary is document state, so it round-trips — and a file that asks for
/// more than the tree supports loads clamped rather than rendering a raster
/// after the view transform.
#[test]
fn boundary_survives_save_load_round_trip() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let _a = effect(&mut engine, "invert");
    let _b = effect(&mut engine, "grain");
    engine.set_screen_space_boundary(2);

    let bytes = save_to_zip(&mut engine, None);

    let mut reloaded = test_engine(16, 16);
    reloaded.open_document(&bytes).expect("round trip loads");
    assert_eq!(
        stored_count(&reloaded),
        2,
        "the boundary survives the file, and the same two nodes are in the run"
    );

    // A hand-edited count past what the tree supports is clamped on load, not
    // trusted: load is the one path that never calls `Document::link`.
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let _one = effect(&mut engine, "invert");
    engine.set_screen_space_boundary(1);
    let overreaching = save_to_zip(&mut engine, Some(99));

    let mut reloaded = test_engine(16, 16);
    reloaded
        .open_document(&overreaching)
        .expect("an overreaching count is clamped, not refused");
    assert_eq!(
        stored_count(&reloaded),
        1,
        "clamped to the qualifying suffix"
    );
}

/// Drive a save to completion and return the `.darkly` bytes, optionally
/// rewriting `screen_space_count` in the manifest on the way out — the stand-in
/// for a hand-edited or future-written file.
fn save_to_zip(engine: &mut DarklyEngine, override_count: Option<u64>) -> Vec<u8> {
    engine
        .start_save_document(SavePurpose::File)
        .expect("save kicks off");
    for _ in 0..32 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if let Some(mut bundle) = engine.poll_save_result() {
            if let Some(count) = override_count {
                let mut manifest: serde_json::Value =
                    serde_json::from_slice(&bundle.manifest_json).expect("manifest parses");
                manifest["screen_space_count"] = serde_json::json!(count);
                bundle.manifest_json = serde_json::to_vec(&manifest).expect("manifest serializes");
            }
            return darkly::format::zip_io::assemble_zip(&bundle);
        }
    }
    panic!("save did not complete within 32 frames");
}
