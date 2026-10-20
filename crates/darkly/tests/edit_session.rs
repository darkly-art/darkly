//! The generic edit-session surface: which pending edit the engine reports
//! as open (`EngineState::edit_session`), and the Apply and Discard that
//! reach it through `apply_edit_session` / `discard_edit_session` without
//! the caller naming its kind.
//!
//! What each kind's Apply and Discard mean is pinned next to that kind
//! (`stroke_edit.rs` for stroke edit); these cells pin the routing.
//!
//! Run with: `cargo test -p darkly --test edit_session --features testing -- --test-threads=1`

mod common;

use common::{event, headless, new_engine, readback};
use darkly::coord::CanvasRect;
use darkly::document::SelectionMode;
use darkly::engine::protocol::RawParams;
use darkly::engine::DarklyEngine;
use darkly::gpu::transform::affine_translate;
use darkly::layer::LayerId;
use darkly::transform::Transform;
use serde_json::json;

const CANVAS: (u32, u32) = (64, 64);

fn session_id(engine: &DarklyEngine) -> Option<&'static str> {
    engine.engine_state().edit_session.map(|kind| kind.id)
}

fn stroke(engine: &mut DarklyEngine, layer: LayerId) {
    headless(
        engine,
        layer,
        &[event(10.0, 10.0, 0.0), event(50.0, 40.0, 16.0)],
    );
}

/// An opaque red `size` square, as RGBA bytes.
fn red_square(size: u32) -> Vec<u8> {
    [255, 0, 0, 255].repeat((size * size) as usize)
}

/// A layer holding an 8x8 red block at (20, 20), selected, so a transform
/// sets up synchronously.
fn selected_block(engine: &mut DarklyEngine) -> LayerId {
    let layer = engine.paste_image(8, 8, &red_square(8), 20, 20, None);
    engine.select_rect(20.0, 20.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    layer
}

fn translate(engine: &mut DarklyEngine, dx: f32) {
    engine.update_floating_matrix(Transform::from_affine(affine_translate(dx, 0.0)));
}

/// With nothing open, the engine reports no session, and Apply and Discard
/// answer `false` and change nothing. A kept stroke nobody is editing is
/// pending but not open.
#[test]
fn nothing_open_reports_no_session() {
    let mut engine = new_engine(CANVAS, "Ink Pen", 0.0);
    assert_eq!(session_id(&engine), None);

    let layer = engine.add_raster_layer(None);
    stroke(&mut engine, layer);
    assert_eq!(session_id(&engine), None, "a kept stroke is not open");
    let pixels = readback(&mut engine, layer);
    let steps = engine.test_undo_len();
    assert!(!engine.apply_edit_session());
    assert!(!engine.discard_edit_session());
    assert_eq!(engine.test_undo_len(), steps);
    assert_eq!(readback(&mut engine, layer), pixels);
}

/// Pasted content is open as `floating`: Discard drops it with the layer
/// the paste made, and Apply lands it as one undo entry.
#[test]
fn floating_paste_session() {
    let mut engine = new_engine(CANVAS, "Ink Pen", 0.0);
    let steps = engine.test_undo_len();

    let pasted = engine.paste_image_floating(8, 8, &red_square(8), 4, 4, None);
    assert_eq!(session_id(&engine), Some("floating"));
    assert!(engine.discard_edit_session());
    assert!(!engine.has_floating());
    assert_eq!(session_id(&engine), None);
    assert!(!engine.has_layer(pasted), "the paste's layer is dropped");
    assert_eq!(engine.test_undo_len(), steps);

    let pasted = engine.paste_image_floating(8, 8, &red_square(8), 4, 4, None);
    translate(&mut engine, 10.0);
    assert!(engine.apply_edit_session());
    assert!(!engine.has_floating());
    assert!(engine.has_layer(pasted));
    assert_eq!(engine.test_undo_len(), steps + 1);
    let layer = readback(&mut engine, pasted);
    assert_eq!(
        alpha_at(&layer, 16, 6),
        255,
        "the paste landed where it was moved"
    );
    assert_eq!(alpha_at(&layer, 6, 6), 0, "and not where it started");
}

/// The alpha of a layer readback at plane `(x, y)`.
fn alpha_at((pixels, bounds): &(Vec<u8>, CanvasRect), x: i32, y: i32) -> u8 {
    let lx = (x - bounds.x0()) as u32;
    let ly = (y - bounds.y0()) as u32;
    pixels[((ly * bounds.width + lx) * 4 + 3) as usize]
}

/// A transform is open as `transform`: Discard leaves the layer as it was,
/// and Apply lands the move as one undo entry.
#[test]
fn transform_session() {
    let mut engine = new_engine(CANVAS, "Ink Pen", 0.0);
    let layer = selected_block(&mut engine);
    let before = readback(&mut engine, layer);
    let steps = engine.test_undo_len();

    assert!(engine.begin_transform(layer));
    assert_eq!(session_id(&engine), Some("transform"));
    translate(&mut engine, 12.0);
    assert!(engine.discard_edit_session());
    assert_eq!(session_id(&engine), None);
    assert_eq!(engine.test_undo_len(), steps);
    assert_eq!(readback(&mut engine, layer), before);

    assert!(engine.begin_transform(layer));
    translate(&mut engine, 12.0);
    assert!(engine.apply_edit_session());
    assert_eq!(session_id(&engine), None);
    assert_eq!(engine.test_undo_len(), steps + 1);
    assert_ne!(readback(&mut engine, layer), before, "the block moved");
}

/// A shape being placed lasts only for its drag, so it is never open: Apply
/// and Discard pass it by, and the next document edit still resolves it.
#[test]
fn shape_placement_is_not_presented() {
    let mut engine = new_engine(CANVAS, "Ink Pen", 0.0);
    let steps = engine.test_undo_len();
    engine.begin_shape(
        "polygon".to_string(),
        RawParams(json!({ "points": 4 })),
        8.0,
        8.0,
        16.0,
        16.0,
        Some([255, 0, 0, 255]),
        None,
        None,
    );
    let layer = engine
        .shape_placement_layer_for_test()
        .expect("placing a shape");
    assert_eq!(session_id(&engine), None);
    assert!(!engine.apply_edit_session());
    assert!(!engine.discard_edit_session());
    assert_eq!(engine.shape_placement_layer_for_test(), Some(layer));

    engine.commit_pending_edits();
    assert_eq!(engine.shape_placement_layer_for_test(), None);
    assert!(engine.has_layer(layer), "resolving keeps the shape");
    assert_eq!(engine.test_undo_len(), steps + 1);
}

/// Stroke edit is open as `strokeEdit`, described by its own kind, and
/// Apply and Discard both close it.
#[test]
fn stroke_edit_session() {
    let mut engine = new_engine(CANVAS, "Ink Pen", 0.0);
    let layer = engine.add_raster_layer(None);
    stroke(&mut engine, layer);

    assert!(engine.stroke_edit_begin());
    let kind = engine.engine_state().edit_session.expect("open");
    assert_eq!(
        (kind.id, kind.display_name, kind.icon),
        ("strokeEdit", "Stroke edit", "fa6-solid:pen-nib")
    );
    assert!(engine.apply_edit_session());
    assert_eq!(session_id(&engine), None);

    assert!(engine.stroke_edit_begin());
    assert!(engine.discard_edit_session());
    assert_eq!(session_id(&engine), None);
}

/// A stroke kept after Apply settles below the session that starts next,
/// and only that session is reported.
#[test]
fn starting_a_session_settles_a_kept_stroke() {
    let mut engine = new_engine(CANVAS, "Ink Pen", 0.0);
    let layer = engine.add_raster_layer(None);
    let steps = engine.test_undo_len();
    stroke(&mut engine, layer);
    engine.stroke_edit_begin();
    engine.apply_edit_session();
    assert_eq!(engine.test_undo_len(), steps, "Apply keeps it pending");

    engine.paste_image_floating(8, 8, &red_square(8), 4, 4, None);
    assert_eq!(engine.test_undo_len(), steps + 1, "the stroke settled");
    assert_eq!(session_id(&engine), Some("floating"));
}
