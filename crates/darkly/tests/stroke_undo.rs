//! A brush stroke's undo entry: what it captures and when.
//!
//! The entry is committed from the stroke's own pre-stroke snapshot over
//! the union of its dabs' write footprints, so these cells pin that the
//! rect covers everything the stroke changed and that nothing which runs
//! between pen-up and the commit can change what the entry holds.
//!
//! Run with: `cargo test -p darkly --test stroke_undo --features testing -- --test-threads=1`

mod common;

use common::{event, headless, new_engine, readback, Vehicle};
use darkly::brush::input_value::InputValue;
use darkly::coord::CanvasRect;
use darkly::engine::{DarklyEngine, SavePurpose};

/// An edit that runs right after pen-up, before any frame, must not leak
/// into the stroke's undo entry: undoing both returns the layer to its
/// pre-stroke pixels.
#[test]
fn edit_right_after_a_stroke_leaves_its_undo_entry_intact() {
    let canvas = (96, 96);
    let mut engine = new_engine(canvas, Vehicle::Builtin("Ink"), 0.0);
    let layer = engine.add_raster_layer(None);
    let (before, _) = readback(&mut engine, layer);

    let ops: Vec<_> = (0..12)
        .map(|i| event(10.0 + i as f32 * 6.0, 48.0, i as f64 * 8.0))
        .collect();
    headless(&mut engine, layer, &ops);
    // No `render` in between: the next edit lands before any frame runs.
    engine.fill_background_color(layer, [40, 160, 220, 255]);

    engine.undo();
    engine.render(0.0);
    engine.undo();
    engine.render(0.0);

    let (after, _) = readback(&mut engine, layer);
    let differing = before
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "undoing the fill and then the stroke must restore the pre-stroke \
         layer; {differing} pixels still differ"
    );
}

/// A mask that grows mid-stroke reads white in the grown region, on screen,
/// wherever no dab landed: growth fills with the mask's default, and the
/// stroke's commit must not overwrite that with an unfilled snapshot.
#[test]
fn mask_grown_mid_stroke_stays_white_outside_the_dabs() {
    let (cw, ch) = (128u32, 128u32);
    let mut engine = new_engine((cw, ch), Vehicle::Builtin("Ink"), 0.0);
    // A non-zero canvas origin exercises the frame math.
    let (ox, oy) = (15i32, 9i32);
    engine.resize_canvas(CanvasRect::from_xywh(ox, oy, cw, ch));

    let layer = engine.add_raster_layer(None);
    engine.add_mask(layer).expect("add mask");
    let mask = engine.host_mask_id(layer).expect("mask exists");
    let before = engine.node_pixel_bounds(mask).expect("mask bounds");

    // A first dab inside the mask, then one past its right edge at the
    // vertical middle, so the mask grows during the stroke.
    let y = (oy + ch as i32 / 2) as f32;
    headless(
        &mut engine,
        mask,
        &[
            event((ox + cw as i32 - 20) as f32, y, 0.0),
            event((ox + cw as i32) as f32 + 40.0, y, 16.0),
        ],
    );
    engine.render(0.0);
    engine.test_flush_readbacks();

    let after = engine.node_pixel_bounds(mask).expect("mask bounds");
    assert!(
        after.width > before.width,
        "test setup: the mask must grow; before {before:?} after {after:?}"
    );

    // The grown columns' top and bottom rows lie far from the horizontal
    // stroke at the middle row.
    let pixels = engine.test_readback_mask(layer);
    let at = |x: i32, y: i32| {
        let lx = (x - after.origin.x) as u32;
        let ly = (y - after.origin.y) as u32;
        pixels[(ly * after.width + lx) as usize]
    };
    let right = after.origin.x + after.width as i32 - 1;
    let top = after.origin.y;
    let bottom = after.origin.y + after.height as i32 - 1;
    for (x, y) in [(right, top), (right, bottom)] {
        let v = at(x, y);
        assert!(
            v > 250,
            "grown mask pixel ({x}, {y}), far from every dab, reads {v}; \
             it must stay white (mask before {before:?}, after {after:?})"
        );
    }
}

/// Dabs that land away from the pen path are still inside the undo entry:
/// its rect is the union of the footprints the terminal published for the
/// dabs it actually drew, not an envelope around the pen's events.
#[test]
fn undo_covers_dabs_placed_away_from_the_pen() {
    let mut engine = new_engine((128, 128), Vehicle::Builtin("Ink"), 0.0);
    // Paint at a fixed point rather than at the pen: every dab lands at
    // (100, 24) while the pen moves along the bottom-left.
    let id = |engine: &darkly::engine::DarklyEngine, type_id: &str| {
        engine
            .active_brush_graph()
            .nodes()
            .values()
            .find(|n| n.type_id == type_id)
            .unwrap_or_else(|| panic!("no '{type_id}' node"))
            .id
            .0
            .clone()
    };
    let (pen, paint) = (id(&engine, "pen_input"), id(&engine, "paint"));
    engine
        .brush_graph_disconnect(&pen, "position", &paint, "position")
        .expect("Ink wires the pen to the dab position");
    engine
        .brush_graph_set_input(&paint, "position", InputValue::Vec2([100.0, 24.0]))
        .expect("position takes a constant");

    let layer = engine.add_raster_layer(None);
    let (before, _) = readback(&mut engine, layer);
    let ops: Vec<_> = (0..8)
        .map(|i| event(10.0 + i as f32 * 4.0, 110.0, i as f64 * 16.0))
        .collect();
    headless(&mut engine, layer, &ops);
    let (painted, bounds) = readback(&mut engine, layer);
    let alpha = |x: u32, y: u32| painted[((y * bounds.width + x) * 4 + 3) as usize];
    assert!(
        alpha(100, 24) > 0,
        "test setup: the dabs land at the fixed point"
    );
    assert_eq!(alpha(20, 110), 0, "test setup: nothing lands at the pen");

    engine.undo();
    engine.render(0.0);
    let (after, _) = readback(&mut engine, layer);
    assert!(
        after == before,
        "undo must remove the dabs placed away from the pen"
    );
}

/// A stroke counts as an unsaved change from pen-up, though its undo entry
/// is pushed only when the stroke settles, and a save in between keeps the
/// document clean through the settle: the settle records no new change.
#[test]
fn a_stroke_is_unsaved_from_pen_up_until_saved() {
    let save = |engine: &mut DarklyEngine| {
        engine
            .start_save_document(SavePurpose::File)
            .expect("save kicks off");
        for _ in 0..16 {
            engine.test_flush_readbacks();
            engine.render(0.0);
            if engine.poll_save_result().is_some() {
                return;
            }
        }
        panic!("save should complete within 16 frames");
    };
    let mut engine = new_engine((64, 64), Vehicle::Builtin("Ink"), 0.0);
    let layer = engine.add_raster_layer(None);
    save(&mut engine);
    assert!(!engine.is_dirty(), "test setup: saved");

    headless(
        &mut engine,
        layer,
        &[event(10.0, 10.0, 0.0), event(50.0, 40.0, 16.0)],
    );
    assert!(engine.is_dirty(), "the stroke is unsaved work at pen-up");

    save(&mut engine);
    let steps = engine.test_undo_len();
    engine.commit_pending_edits();
    assert_eq!(engine.test_undo_len(), steps + 1, "the stroke settled");
    assert!(
        !engine.is_dirty(),
        "settling a saved stroke changes nothing unsaved"
    );
}
