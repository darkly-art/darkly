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
use darkly::gpu::params::ParamValue;
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
    effect_anchored(engine, pipeline, None)
}

/// [`effect`] placed relative to an existing node, the way the add-layer modal
/// anchors on the active layer.
fn effect_anchored(engine: &mut DarklyEngine, pipeline: &str, anchor: Option<LayerId>) -> LayerId {
    let defaults: Vec<_> = engine
        .filter_param_defs(pipeline)
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect();
    engine
        .add_filter_layer(pipeline, defaults, anchor)
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

/// Run members, bottom-to-top. The response is top-first with the divider as
/// one of its rows, so the run is everything before the divider row, reversed.
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

fn stored_count(engine: &DarklyEngine) -> usize {
    run_ids(engine).len()
}

fn in_run(engine: &DarklyEngine, id: LayerId) -> bool {
    run_ids(engine).contains(&id)
}

/// The divider's id, read off the same layer-tree response the panel consumes.
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

/// Root rows, top-first — the panel's order.
fn root_rows(engine: &DarklyEngine) -> Vec<LayerId> {
    tree_json(engine)["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .map(row_id)
        .collect()
}

/// Children of a root-level group row, top-first — the panel's order.
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

/// A root-level group row's stored passthrough flag, as the panel sees it.
fn passthrough_flag(engine: &DarklyEngine, group: LayerId) -> bool {
    tree_json(engine)["layers"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row_id(row) == group)
        .expect("group row")["passthrough"]
        .as_bool()
        .expect("passthrough flag")
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

    engine.test_set_screen_space_boundary(2);
    assert_eq!(
        run_ids(&engine),
        vec![a, b],
        "both effects are viewport-only"
    );
    assert!(!in_run(&engine, raster));

    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![b], "only the topmost stays above");

    engine.test_set_screen_space_boundary(0);
    assert!(run_ids(&engine).is_empty(), "the run empties");
    assert_eq!(
        root_row_count(&engine),
        4,
        "and everything is canvas-space again — three nodes plus the divider row"
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

    engine.test_set_screen_space_boundary(1);
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
    engine.test_set_screen_space_boundary(1);
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
    engine.test_set_screen_space_boundary(3);
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

    // An explicit move that targets the top of the stack. Unlike every path
    // above it, a move states an intent about *placement* — so it is refused
    // outright rather than quietly landed somewhere else.
    let err = engine
        .move_layers(vec![anchored], MoveTarget::After(e3))
        .expect_err("an explicit move above the run is refused");
    assert!(
        err.contains("viewport space"),
        "the refusal explains itself: {err}"
    );
    assert_eq!(run_ids(&engine), expected, "move_layers above the run");
    assert!(!in_run(&engine, anchored));
}

/// The structural clauses of the boundary rules, enforced loudly at mutation
/// time: an overshooting divider move is refused naming its blocker, and a
/// mask cannot attach to a run member at all. (The count design's read clamp
/// degraded both cases silently; the divider node refuses instead.)
#[test]
fn masked_or_isolated_nodes_cannot_be_above_the_boundary() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    let divider = divider_row_id(&engine);

    // A raster can never sit above the boundary, so the divider move that
    // would put it there is refused, naming it.
    let err = engine
        .move_layer(divider, MoveTarget::Before(raster))
        .expect_err("moving the divider below a raster is refused");
    assert!(
        err.contains("viewport boundary"),
        "the refusal explains itself: {err}"
    );
    assert!(
        run_ids(&engine).is_empty(),
        "the refused move moved nothing"
    );

    // Over the effect alone it is an ordinary move.
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![e]);

    // A mask cannot attach to a run member: attaching is what would have
    // disqualified the host in place, so it refuses loudly.
    let err = engine
        .add_mask(e)
        .expect_err("a mask on a run member is refused");
    assert!(err.contains("mask"), "the refusal explains itself: {err}");
    assert_eq!(run_ids(&engine), vec![e], "the run is untouched");
    // On a canvas-space host the same call still works.
    engine.add_mask(raster).expect("mask on a canvas layer");
}

/// A group crosses the line whole, and only when everything inside it can.
#[test]
fn a_group_is_eligible_exactly_when_its_contents_are() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    let group = engine.group_layers(vec![e]).expect("group of one effect");

    engine.test_set_screen_space_boundary(1);
    assert_eq!(
        run_ids(&engine),
        vec![group],
        "a group of effects is eligible"
    );

    // Isolation is a canvas-space notion; above the divider the run is
    // consumed flattened, so the flag changes nothing up there.
    engine.set_group_passthrough(group, false);
    assert_eq!(
        run_ids(&engine),
        vec![group],
        "the passthrough flag does not gate eligibility"
    );
    engine.set_group_passthrough(group, true);

    // A group in the run may only hold what can render there, and a move into
    // it is refused rather than silently emptying the run.
    let err = engine
        .move_layers(vec![raster], MoveTarget::IntoGroupTop(group))
        .expect_err("a raster may not be moved into a run group");
    assert!(
        err.contains("viewport space"),
        "the refusal explains itself: {err}"
    );
    assert_eq!(
        run_ids(&engine),
        vec![group],
        "and the refused move left the arrangement alone"
    );
}

/// Eligibility is structural. Toggling an eye must never change what a document
/// exports.
#[test]
fn visibility_never_changes_which_space_a_node_is_in() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let canvas_effect = effect(&mut engine, "invert");
    engine.add_mask(canvas_effect).expect("add mask");
    let run_effect = effect(&mut engine, "grain");
    engine.test_set_screen_space_boundary(1);
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

    engine.test_set_screen_space_boundary(2);
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
    engine.test_set_screen_space_boundary(2);
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
    engine.test_set_screen_space_boundary(2);

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
    engine.test_set_screen_space_boundary(1);
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

// ---------------------------------------------------------------------------
// Instance reuse
// ---------------------------------------------------------------------------

/// An effect instance is built once and reused. Rebuilding one means recompiling
/// its pipeline lookup, re-running `ScaledEffect::prepare` and allocating fresh
/// bind groups; doing that per frame is invisible except as lag while painting,
/// which is exactly how it was found.
#[test]
fn effect_instances_are_not_rebuilt_every_frame() {
    let mut engine = test_engine(64, 64);
    let raster = engine.add_raster_layer(None);
    fill_layer(&mut engine, raster, 255, 0, 0);
    let _canvas_effect = effect(&mut engine, "invert");
    let _screen_effect = effect(&mut engine, "grain");
    engine.test_set_screen_space_boundary(1);

    // Settle: the first frames legitimately build both instances.
    for _ in 0..4 {
        engine.render(0.0);
    }
    let settled = engine.test_effect_rebuilds();

    // Now paint, which dirties the composite every frame — the case that was
    // slow. Nothing structural changes, so nothing may be rebuilt.
    for i in 0..8 {
        engine.begin_stroke(raster).unwrap();
        engine.stroke_to(StrokeOp::BrushStroke {
            x: 10.0 + i as f32,
            y: 10.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: 0.0,
            cr: 0.0,
            cg: 0.0,
            cb: 1.0,
            ca: 1.0,
        });
        engine.end_stroke();
        engine.render(0.0);
    }

    assert_eq!(
        engine.test_effect_rebuilds(),
        settled,
        "painting must not rebuild any effect instance"
    );
}

/// Dragging a run member down next to a canvas-space layer takes it out of the
/// run.
///
/// Crossing the divider does not change a node's index — the lowest
/// viewport-only child and the topmost canvas child are the same position — so
/// the order of the children cannot express the move. What does express it is
/// *what the node was dropped next to*: a layer dropped beside a canvas-space
/// layer is canvas-space. Reported as "dragging a veil beneath the viewport
/// boundary doesn't work"; it was a silent no-op.
#[test]
fn dragging_a_run_member_below_the_divider_takes_it_out_of_the_run() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let bottom = effect(&mut engine, "rainy_glass");
    let middle = effect(&mut engine, "grain");
    let top = effect(&mut engine, "vhs");
    engine.test_set_screen_space_boundary(3);
    assert_eq!(run_ids(&engine), vec![bottom, middle, top]);

    // The gesture: drop the lowest run member just above the raster — the slot
    // directly below the divider, which is the position it already occupies.
    engine
        .move_layers(vec![bottom], MoveTarget::After(raster))
        .expect("move below the divider");

    assert!(
        !in_run(&engine, bottom),
        "the dragged effect must end up below the line"
    );
    assert_eq!(
        run_ids(&engine),
        vec![middle, top],
        "and the rest of the run is undisturbed"
    );
}

/// The mirror gesture: dropping a canvas-space effect beside a run member puts
/// it in the run. Same reference-node rule, opposite outcome.
#[test]
fn dragging_an_effect_above_the_divider_puts_it_in_the_run() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let below = effect(&mut engine, "rainy_glass");
    let above = effect(&mut engine, "grain");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![above]);

    engine
        .move_layers(vec![below], MoveTarget::Before(above))
        .expect("move above the divider");

    assert_eq!(
        run_ids(&engine),
        vec![below, above],
        "the dragged effect joins the run without moving anything else"
    );
}

// ---------------------------------------------------------------------------
// Animation across the divider
// ---------------------------------------------------------------------------

/// Grain at full reshuffle rate: `needs_animation()` is true, and every tick
/// reseeds the noise, so two frames taken across a tick cannot be
/// byte-identical.
fn animated_grain(engine: &mut DarklyEngine) -> LayerId {
    engine
        .add_filter_layer(
            "grain",
            vec![
                ParamValue::Float(1.0), // speed
                ParamValue::Float(0.0), // color
                ParamValue::Float(1.0), // opacity
            ],
            None,
        )
        .expect("`grain` should be addable as an effect layer")
}

/// Drain startup async work and clear the flags `frame_needs_more` reports
/// independently of animation demand — headless renders never reach
/// `finish_present`, so `needs_present` would otherwise stay stuck set from
/// engine setup. What `frame_needs_more` returns afterwards is the animation
/// answer alone.
fn quiesce(engine: &mut DarklyEngine) {
    for _ in 0..8 {
        engine.render(0.0);
    }
    engine.test_flush_readbacks();
    engine.test_clear_needs_present();
}

/// Advance the master clock past several divisor boundaries. `frame_count`
/// only moves here in a headless engine, so the divisor-2 defaults are crossed
/// deterministically.
fn tick_animations(engine: &mut DarklyEngine) {
    engine.test_tick_animations(1.0); // primes last_wall_time; dt == 0
    for i in 1..=8 {
        engine.test_tick_animations(1.0 + i as f32 * 0.05);
    }
}

/// Regression: a canvas-space animated effect is the document's only animated
/// content. It must keep the frame loop alive and advance its clock across
/// frames — the canvas tick and `needs_animation()` were both gated on animated
/// *voids* only, so the effect froze unless a void coincidentally existed.
#[test]
fn canvas_space_animated_effect_animates() {
    let mut engine = test_engine(32, 32);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);

    // Baseline quiescence *before* the effect exists, so the assertion below
    // can only be satisfied by the effect layer, not by a leftover flag.
    quiesce(&mut engine);
    assert!(
        !engine.test_frame_needs_more(),
        "engine must be quiescent before the animated effect is added"
    );

    // The boundary is at 0, so the effect is canvas-space.
    let fx = animated_grain(&mut engine);

    // Composite once — that realizes and syncs the effect instance — then clear
    // the transient flags again.
    let before = engine.test_readback_canvas();
    engine.test_flush_readbacks();
    engine.test_clear_needs_present();

    assert!(
        engine.test_frame_needs_more(),
        "a visible canvas-space animated effect must keep the frame loop alive"
    );

    tick_animations(&mut engine);
    assert_ne!(
        before,
        engine.test_readback_canvas(),
        "ticking the clock must advance a canvas-space effect and re-composite; \
         identical bytes mean the canvas gate never fired"
    );

    // Hiding it silences the loop — the predicate honors effective visibility
    // like every other animation gate. The hide is itself a document change
    // that owes one frame; a headless engine has no surface to present on, so
    // absorb that debt the same way the composite above was absorbed, leaving
    // the animation predicate as the only thing under test.
    engine.set_layer_visible(fx, false);
    engine.test_clear_needs_present();
    assert!(
        !engine.test_frame_needs_more(),
        "a hidden canvas-space animated effect must not keep the loop alive"
    );
}

/// The mirror case, which is what pins both spaces to one mechanism rather than
/// two: the same effect above the divider animates too. This passed before the
/// canvas gate was fixed and must keep passing after — it is the coverage for
/// the screen predicate's enumeration source moving from the document's
/// screen-space run to the realized instance's own space tag.
#[test]
fn screen_space_animated_effect_animates() {
    let (vw, vh) = (32u32, 32u32);
    let mut engine = test_engine(vw, vh);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);
    let fx = animated_grain(&mut engine);
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![fx], "the effect is viewport-only");

    quiesce(&mut engine);

    // A headless engine never realizes a Screen instance until the run has been
    // sized, so this readback is also what puts the instance in the map.
    let before = engine.test_readback_screen_run(vw, vh);
    engine.test_flush_readbacks();
    engine.test_clear_needs_present();

    assert!(
        engine.test_frame_needs_more(),
        "a visible screen-space animated effect must keep the frame loop alive"
    );

    tick_animations(&mut engine);
    assert_ne!(
        before,
        engine.test_readback_screen_run(vw, vh),
        "ticking the clock must advance a screen-space effect"
    );
}

/// Reordering inside the run keeps everything in it: the reference node is a
/// run member, so the moved node inherits that side.
#[test]
fn reordering_inside_the_run_keeps_every_member() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let a = effect(&mut engine, "rainy_glass");
    let b = effect(&mut engine, "grain");
    let c = effect(&mut engine, "vhs");
    engine.test_set_screen_space_boundary(3);

    engine
        .move_layers(vec![c], MoveTarget::Before(a))
        .expect("reorder inside the run");

    assert_eq!(
        run_ids(&engine),
        vec![c, a, b],
        "reordering inside the run keeps all three above the line"
    );
}

// ---------------------------------------------------------------------------
// Groups above the divider
// ---------------------------------------------------------------------------

/// A group above the divider is passthrough, unmasked and holds only effects,
/// so it contributes no compositing of its own — the effects inside it are the
/// run as far as the present chain is concerned. Eligibility already said so
/// (`a_group_is_eligible_exactly_when_its_contents_are`); this pins that the
/// pixels agree.
#[test]
fn an_effect_inside_a_run_group_still_runs_on_the_presented_image() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);
    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);

    let inv = effect(&mut engine, "invert");
    let group = engine.group_layers(vec![inv]).expect("group of one effect");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![group], "the group is the run");
    settle(&mut engine);

    let center = px(&engine.test_readback_screen_run(cw, ch), cw, 8, 8);
    assert!(
        center[0] < 64 && center[1] > 190 && center[2] > 190,
        "an invert nested in a run group must show on the surface: red → cyan, got {center:?}"
    );

    assert_eq!(
        px(&engine.test_readback_canvas(), cw, 8, 8),
        [255, 0, 0, 255],
        "and the exported image is still red"
    );
}

/// The flattened run must interleave a group's effects with its root-level
/// siblings in document order — a group is a container, not a separate chain.
/// Two inverts, one nested and one not, cancel; if the nested one were dropped
/// or run against the wrong pair, the surface would stay inverted.
#[test]
fn a_run_group_flattens_into_the_chain_in_document_order() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);
    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);

    let nested = effect(&mut engine, "invert");
    let group = engine
        .group_layers(vec![nested])
        .expect("group of one effect");
    let sibling = effect(&mut engine, "invert");
    engine.test_set_screen_space_boundary(2);
    assert_eq!(
        run_ids(&engine),
        vec![group, sibling],
        "both the group and the bare effect are in the run"
    );
    settle(&mut engine);

    let center = px(&engine.test_readback_screen_run(cw, ch), cw, 8, 8);
    assert!(
        center[0] > 190 && center[1] < 64 && center[2] < 64,
        "two inverts across the group boundary cancel: still red, got {center:?}"
    );
}

/// Grouping run members is how a viewport effect group gets built, so the new
/// group has to inherit the topmost source's side of the divider. It used to
/// hardcode canvas space, which silently dropped the whole arrangement.
#[test]
fn grouping_run_members_keeps_the_group_in_the_run() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let a = effect(&mut engine, "invert");
    let b = effect(&mut engine, "grain");
    engine.test_set_screen_space_boundary(2);
    assert_eq!(
        run_ids(&engine),
        vec![a, b],
        "both effects start in the run"
    );

    let group = engine.group_layers(vec![a, b]).expect("group both effects");

    assert_eq!(
        run_ids(&engine),
        vec![group],
        "the group inherits the run membership of what it replaced"
    );
    assert_eq!(
        engine.test_screen_space_effects(),
        vec![a, b],
        "and the effects it holds are still what the present chain runs"
    );
}

/// The two refusals the report asked for, stated as the user states them: a
/// group carrying something that cannot render in viewport space may not go
/// there, and nothing that cannot render there may go into a group that has.
#[test]
fn moving_a_group_holding_a_raster_into_viewport_space_is_refused() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![e]);

    // A group of an effect *and* a raster — eligible but for the raster.
    let group = engine
        .group_layers(vec![raster])
        .expect("group the raster alone");
    let before = tree_json(&engine);

    let err = engine
        .move_layers(vec![group], MoveTarget::After(e))
        .expect_err("a group holding a raster may not go above the divider");
    assert!(
        err.contains("viewport space") && err.contains("contains"),
        "the refusal names the offending descendant: {err}"
    );
    assert_eq!(run_ids(&engine), vec![e], "the run is untouched");
    assert_eq!(
        tree_json(&engine),
        before,
        "and a refused move changes nothing at all"
    );
}

/// A refusal must not cost the user their undo history — it never got as far as
/// pushing one, so the stack still holds whatever came before.
#[test]
fn a_legal_viewport_arrangement_survives_undo_and_redo() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let a = effect(&mut engine, "invert");
    let b = effect(&mut engine, "grain");
    let group = engine.group_layers(vec![a, b]).expect("group both effects");
    engine.test_set_screen_space_boundary(1);

    let arranged = run_ids(&engine);
    let chain = engine.test_screen_space_effects();
    assert_eq!(arranged, vec![group]);
    assert_eq!(chain, vec![a, b]);

    // Move the group down and back through undo.
    engine.test_set_screen_space_boundary(0);
    assert!(run_ids(&engine).is_empty());
    engine.undo();
    assert_eq!(run_ids(&engine), arranged, "undo restores the run");
    assert_eq!(
        engine.test_screen_space_effects(),
        chain,
        "including everything the group holds"
    );

    engine.redo();
    assert!(run_ids(&engine).is_empty(), "redo takes it back down");
    engine.undo();
    assert_eq!(run_ids(&engine), arranged, "and undo brings it back again");
    assert_eq!(engine.test_screen_space_effects(), chain);
}

/// An add states no intent about placement, so it is never refused — it lands
/// at the nearest slot the rules allow. For a raster anchored inside a run
/// group that is the topmost canvas-space slot; for an effect it is exactly
/// where it was asked to go.
#[test]
fn adding_into_a_run_group_lands_at_the_nearest_legal_slot() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let a = effect(&mut engine, "invert");
    let group = engine.group_layers(vec![a]).expect("group the effect");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![group]);

    // Anchored on the effect inside the group — the slot asked for is above
    // the divider, which a raster may not occupy. Had it landed there the
    // group would have been disqualified and the run would be empty, so the
    // run surviving intact is what proves the redirect happened.
    engine.add_raster_layer(Some(a));
    assert_eq!(
        run_ids(&engine),
        vec![group],
        "the raster went somewhere legal and the arrangement is intact"
    );
    assert_eq!(
        engine.test_screen_space_effects(),
        vec![a],
        "the group still holds only the effect"
    );

    // An effect asked for the same slot belongs there, and gets it —
    // joining the chain is only possible from inside the group.
    let nested = effect_anchored(&mut engine, "grain", Some(a));
    assert_eq!(
        engine.test_screen_space_effects(),
        vec![a, nested],
        "an effect lands inside the group, unredirected, and joins the chain"
    );
    assert_eq!(run_ids(&engine), vec![group], "the group is still the run");
}

/// Emptiness is not a disqualification. A group whose last effect is deleted
/// stays where it is, and moving an empty group up is not an error: it renders
/// nothing, so there is nothing it can render in the wrong space.
#[test]
fn an_empty_group_is_allowed_above_the_boundary() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let a = effect(&mut engine, "invert");
    let group = engine.group_layers(vec![a]).expect("group the effect");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![group]);

    // Delete the only effect it holds. The group is now empty and still in the
    // run, rather than being evicted the moment it has nothing to do.
    engine.remove_layers(vec![a]).expect("delete the effect");
    assert_eq!(
        run_ids(&engine),
        vec![group],
        "an emptied group keeps its place above the divider"
    );
    assert!(
        engine.test_screen_space_effects().is_empty(),
        "and contributes nothing to the present chain"
    );

    // Undo brings the effect back, and with it the chain.
    engine.undo();
    assert_eq!(run_ids(&engine), vec![group]);
    assert_eq!(engine.test_screen_space_effects(), vec![a]);
}

/// …but an empty group has no claim on where the divider sits, so creating one
/// while a run exists must not sweep it above the line. This is the path that
/// let a fresh group be swept up and then filled with a raster.
#[test]
fn a_freshly_created_group_is_never_swept_into_the_run() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![e]);

    let empty = engine.add_group(None);
    assert!(
        !in_run(&engine, empty),
        "a new empty group lands below the divider, not above it"
    );
    assert_eq!(run_ids(&engine), vec![e], "and the run is unchanged");

    // The same, through the gesture that actually creates groups.
    let wrapped = engine.group_layers(vec![raster]).expect("group the raster");
    assert!(!in_run(&engine, wrapped));
    assert_eq!(run_ids(&engine), vec![e]);
    assert_eq!(
        stored_count(&engine),
        1,
        "the stored intent is not polluted"
    );
}

// ---------------------------------------------------------------------------
// Drag placement — a move is a statement about position, and the engine
// honors it verbatim
// ---------------------------------------------------------------------------

/// A group of effects dragged from canvas space to a slot inside the run lands
/// at that slot.
#[test]
fn dragging_a_group_into_the_run_lands_where_dropped() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let g_eff = effect(&mut engine, "invert");
    let e1 = effect(&mut engine, "grain");
    let e2 = effect(&mut engine, "vhs");
    engine.test_set_screen_space_boundary(2);
    assert_eq!(run_ids(&engine), vec![e1, e2]);

    let group = engine.group_layers(vec![g_eff]).expect("group the effect");
    assert_eq!(run_ids(&engine), vec![e1, e2], "grouping changed nothing");

    // Panel gesture: drop the group directly above e1's row (below e2).
    engine
        .move_layers(vec![group], MoveTarget::After(e1))
        .expect("move into the run");

    assert_eq!(
        run_ids(&engine),
        vec![e1, group, e2],
        "the group lands between e1 and e2, where it was dropped"
    );
}

/// The same drop with an *empty* group. Emptiness gives it no claim on the
/// divider, but the drop's reference already states the side, and the position
/// is the user's — it must not be redirected to the run's floor.
#[test]
fn dragging_an_empty_group_into_the_run_lands_where_dropped() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e1 = effect(&mut engine, "grain");
    let e2 = effect(&mut engine, "vhs");
    engine.test_set_screen_space_boundary(2);
    assert_eq!(run_ids(&engine), vec![e1, e2]);

    let group = engine.add_group(None);
    assert!(!in_run(&engine, group), "a new group starts below");

    // Panel gesture: drop the empty group above e2's row, the run's top.
    engine
        .move_layers(vec![group], MoveTarget::After(e2))
        .expect("move into the run");

    assert_eq!(
        run_ids(&engine),
        vec![e1, e2, group],
        "the empty group lands above e2, where it was dropped"
    );
}

/// Undo and redo of that drop restore the recorded slot exactly — redo must
/// not replay through any placement guess.
#[test]
fn empty_group_run_move_round_trips_through_undo_and_redo() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e1 = effect(&mut engine, "grain");
    let e2 = effect(&mut engine, "vhs");
    engine.test_set_screen_space_boundary(2);
    let group = engine.add_group(None);
    engine
        .move_layers(vec![group], MoveTarget::After(e2))
        .expect("move into the run");
    assert_eq!(run_ids(&engine), vec![e1, e2, group]);

    engine.undo();
    assert_eq!(run_ids(&engine), vec![e1, e2], "undo puts the run back");
    assert!(!in_run(&engine, group));

    engine.redo();
    assert_eq!(
        run_ids(&engine),
        vec![e1, e2, group],
        "redo lands at the recorded slot, not a redirected one"
    );
}

/// An effect dropped at a slot inside a run group lands at that slot.
#[test]
fn dragging_an_effect_into_a_run_group_lands_where_dropped() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let e_b = effect(&mut engine, "vhs");
    let group = engine.group_layers(vec![e_a, e_b]).expect("group");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![group]);
    assert_eq!(group_children(&engine, group), vec![e_b, e_a]);

    let e_c = effect(&mut engine, "invert");
    // Panel gesture: drop above e_b's row, the group's top.
    engine
        .move_layers(vec![e_c], MoveTarget::After(e_b))
        .expect("move into the group");

    assert_eq!(
        group_children(&engine, group),
        vec![e_c, e_b, e_a],
        "e_c lands above e_b, where it was dropped"
    );
}

/// Dropping onto a group header's body targets the group's top.
#[test]
fn dropping_onto_a_run_group_header_lands_on_top() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let e_b = effect(&mut engine, "vhs");
    let group = engine.group_layers(vec![e_a, e_b]).expect("group");
    engine.test_set_screen_space_boundary(1);

    let e_c = effect(&mut engine, "invert");
    engine
        .move_layers(vec![e_c], MoveTarget::IntoGroupTop(group))
        .expect("move into the group");

    assert_eq!(
        group_children(&engine, group),
        vec![e_c, e_b, e_a],
        "into-top means the panel top"
    );
}

/// `before group` is a sibling slot below the whole group — never a slot
/// inside it. The panel's collapsed-header below-zone means exactly this.
#[test]
fn before_group_is_a_sibling_below_the_whole_group() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let e_b = effect(&mut engine, "vhs");
    let group = engine.group_layers(vec![e_a, e_b]).expect("group");
    engine.test_set_screen_space_boundary(1);

    let e_c = effect(&mut engine, "invert");
    engine
        .move_layers(vec![e_c], MoveTarget::Before(group))
        .expect("move before the group");

    assert!(
        !group_children(&engine, group).contains(&e_c),
        "before-group lands outside the group"
    );
    let rows = root_rows(&engine);
    let gi = rows.iter().position(|&r| r == group).unwrap();
    assert_eq!(rows[gi + 1], e_c, "e_c sits directly below the group block");
}

/// A paste is an add, not a move: anchored on a run member, the new raster is
/// placed at the nearest legal slot — the canvas floor — and the run survives.
#[test]
fn pasting_with_a_run_member_anchor_preserves_the_run() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e = effect(&mut engine, "invert");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![e]);

    let pasted = engine.paste_image(4, 4, &[255u8; 4 * 4 * 4], 0, 0, Some(e));

    assert_eq!(run_ids(&engine), vec![e], "the run is untouched");
    assert!(!in_run(&engine, pasted));
    // Rows top-first: the effect, the divider row, then the redirected paste.
    assert_eq!(
        root_rows(&engine)[2],
        pasted,
        "the pasted raster sits directly below the divider"
    );
}

// ---------------------------------------------------------------------------
// Passthrough above the divider
// ---------------------------------------------------------------------------

/// A group's passthrough flag is meaningless above the divider — the run is
/// consumed flattened — so unchecking it must not silently drop the group's
/// effects out of the present chain.
#[test]
fn unchecking_passthrough_on_a_run_group_keeps_effects_running() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "grain");
    let group = engine.group_layers(vec![e_a]).expect("group");
    engine.test_set_screen_space_boundary(1);
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

/// The flag being irrelevant up there cuts both ways: an isolated group of
/// effects may be dragged into the run, where it renders flattened.
#[test]
fn an_isolated_group_of_effects_may_enter_the_run() {
    let mut engine = test_engine(16, 16);
    let _raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "invert");
    let group = engine.group_layers(vec![e_a]).expect("group");
    engine.set_group_passthrough(group, false);
    let e_top = effect(&mut engine, "grain");
    engine.test_set_screen_space_boundary(1);
    assert_eq!(run_ids(&engine), vec![e_top]);

    engine
        .move_layers(vec![group], MoveTarget::Before(e_top))
        .expect("an isolated group of effects is allowed above the line");

    assert_eq!(run_ids(&engine), vec![group, e_top]);
    assert_eq!(
        engine.test_screen_space_effects(),
        vec![e_a, e_top],
        "its effects flatten into the chain"
    );
}

/// The stored flag is intent, not state the divider owns: it survives the trip
/// through the run untouched and re-applies isolation below the line.
#[test]
fn passthrough_flag_survives_the_run_and_reapplies_below() {
    let mut engine = test_engine(16, 16);
    let raster = engine.add_raster_layer(None);
    let e_a = effect(&mut engine, "invert");
    let group = engine.group_layers(vec![e_a]).expect("group");
    engine.set_group_passthrough(group, false);
    let e_top = effect(&mut engine, "grain");
    engine.test_set_screen_space_boundary(1);

    engine
        .move_layers(vec![group], MoveTarget::Before(e_top))
        .expect("move into the run");
    assert!(in_run(&engine, group));
    assert!(
        !passthrough_flag(&engine, group),
        "the flag is ignored above the divider, not cleared"
    );

    engine
        .move_layers(vec![group], MoveTarget::After(raster))
        .expect("move back below the divider");
    assert!(!in_run(&engine, group));
    assert!(
        !passthrough_flag(&engine, group),
        "back in canvas space the group is isolated again, as stored"
    );
}
