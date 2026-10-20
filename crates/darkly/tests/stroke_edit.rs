//! Stroke edit: the last brush stroke, redrawn from its raw pen events with
//! the brush and colour as they now are, until it settles into one undo
//! entry. Apply leaves the edit and keeps the stroke editable; Discard puts
//! back the stroke as it looked when the edit was entered.
//!
//! The oracle throughout is a fresh engine painting the same events with
//! the edited brush from the start, at the same seed: a redraw must land
//! exactly what that stroke would have. `stroke_rewind.rs` pins the
//! whole-at-pen-up render these strokes use against a frame-per-event one.
//!
//! Run with: `cargo test -p darkly --test stroke_edit --features testing -- --test-threads=1`

mod common;

use common::{assert_same, event, headless, install_builtin, new_engine, readback, set_input};
use darkly::brush::color_jitter;
use darkly::config::{self, ConfigValue};
use darkly::coord::CanvasRect;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::{GpuContext, GpuDevice};
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;
use std::sync::Arc;

const CANVAS: (u32, u32) = (128, 128);
const SEED: u32 = 0x5EED;

/// A curved stroke whose last events run past the right edge, so the layer
/// grows during it.
fn curve() -> Vec<StrokeOp> {
    (0..24)
        .map(|i| {
            let t = i as f32 / 23.0;
            event(
                16.0 + t * 140.0,
                64.0 + 30.0 * (t * std::f32::consts::PI).sin(),
                i as f64 * 16.0,
            )
        })
        .collect()
}

/// An engine with `brush` at stabilize 0, a pinned seed and a fresh layer.
fn engine_with(brush: &str) -> (DarklyEngine, LayerId) {
    let mut engine = new_engine(CANVAS, brush, 0.0);
    engine.set_stroke_seed(Some(SEED));
    let layer = engine.add_raster_layer(None);
    (engine, layer)
}

/// What a fresh engine paints: `brush`, configured by `setup` before the
/// stroke, painting `ops`.
fn fresh(
    brush: &str,
    setup: impl FnOnce(&mut DarklyEngine),
    ops: &[StrokeOp],
) -> (Vec<u8>, CanvasRect) {
    let (mut engine, layer) = engine_with(brush);
    setup(&mut engine);
    headless(&mut engine, layer, ops);
    readback(&mut engine, layer)
}

fn editing(engine: &DarklyEngine) -> bool {
    engine
        .engine_state()
        .edit_session
        .is_some_and(|kind| kind.id == "strokeEdit")
}

/// The Ink Pen stroke `curve()` paints at `size`, from a fresh engine.
fn fresh_at(size: f32) -> (Vec<u8>, CanvasRect) {
    fresh(
        "Ink Pen",
        |e| set_input(e, "brush_settings", "size", size),
        &curve(),
    )
}

/// Entering stroke edit with nothing changed redraws the stroke as it was;
/// a size change then redraws it at the new size, and that equals a stroke
/// painted at the new size from the start, grown layer included.
#[test]
fn size_change_redraws_and_matches_fresh_stroke() {
    let ops = curve();
    let (mut engine, layer) = engine_with("Ink Pen");
    set_input(&mut engine, "brush_settings", "size", 0.2);
    headless(&mut engine, layer, &ops);
    let original = readback(&mut engine, layer);

    assert!(engine.stroke_edit_begin(), "a stroke was just painted");
    assert!(editing(&engine));
    assert_same(
        "entering stroke edit",
        "the redraw",
        readback(&mut engine, layer),
        "the original stroke",
        original.clone(),
    );

    set_input(&mut engine, "brush_settings", "size", 0.5);
    let edited = readback(&mut engine, layer);
    assert_ne!(
        edited.0, original.0,
        "the size change must redraw the stroke"
    );
    assert_same(
        "size 0.2 edited to 0.5",
        "the edited stroke",
        edited,
        "a fresh stroke at 0.5",
        fresh(
            "Ink Pen",
            |e| set_input(e, "brush_settings", "size", 0.5),
            &ops,
        ),
    );
}

/// The stabilizer re-runs from the raw events, so a stabilizer change
/// reshapes the stroke.
#[test]
fn stabilizer_change_reshapes_the_stroke() {
    let ops = curve();
    let (mut engine, layer) = engine_with("Ink Pen");
    headless(&mut engine, layer, &ops);
    let unstabilized = readback(&mut engine, layer);

    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "stabilize", 0.5);
    let edited = readback(&mut engine, layer);
    assert_ne!(
        edited.0, unstabilized.0,
        "stabilizing must reshape the stroke"
    );
    assert_same(
        "stabilize 0 edited to 0.5",
        "the edited stroke",
        edited,
        "a fresh stroke at 0.5",
        fresh(
            "Ink Pen",
            |e| set_input(e, "brush_settings", "stabilize", 0.5),
            &ops,
        ),
    );
}

/// Dab orientation follows the view rotation the stroke was painted at,
/// not the one the view has when the stroke is redrawn.
#[test]
fn redraw_keeps_the_strokes_view_rotation() {
    let rotated = |e: &mut DarklyEngine, rotation: f32| {
        let (w, h) = (CANVAS.0 as f32, CANVAS.1 as f32);
        e.set_view_transform(0.0, 0.0, 1.0, rotation, false, w, h, 1.0);
    };
    let ops = curve();
    let (mut engine, layer) = engine_with("Calligraphy");
    headless(&mut engine, layer, &ops);

    rotated(&mut engine, 0.9);
    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.2);
    let edited = readback(&mut engine, layer);

    let at_new_rotation = fresh(
        "Calligraphy",
        |e| {
            rotated(e, 0.9);
            set_input(e, "brush_settings", "size", 0.2);
        },
        &ops,
    );
    assert_ne!(
        edited.0, at_new_rotation.0,
        "test setup: the nib's orientation must depend on the view rotation"
    );
    assert_same(
        "rotated between pen-up and the edit",
        "the edited stroke",
        edited,
        "a fresh stroke at the original rotation",
        fresh(
            "Calligraphy",
            |e| set_input(e, "brush_settings", "size", 0.2),
            &ops,
        ),
    );
}

/// A foreground colour change redraws the stroke in the new colour, and the
/// seed's colour jitter offsets carry over unchanged.
#[test]
fn colour_change_redraws_with_the_same_jitter() {
    let set_jitter = |amount: f64| {
        for key in [
            color_jitter::HUE_KEY,
            color_jitter::SATURATION_KEY,
            color_jitter::VALUE_KEY,
        ] {
            config::set(key, ConfigValue::Float(amount));
        }
    };
    let blue = [0.1, 0.3, 0.9, 1.0];
    for jitter in [0.0, 0.3] {
        set_jitter(jitter);
        let ops = curve();
        let (mut engine, layer) = engine_with("Ink Pen");
        headless(&mut engine, layer, &ops);
        let black = readback(&mut engine, layer);

        engine.stroke_edit_begin();
        engine.set_foreground_color(blue);
        let edited = readback(&mut engine, layer);
        assert_ne!(
            edited.0, black.0,
            "the colour change must redraw (jitter {jitter})"
        );
        assert_same(
            &format!("black edited to blue, jitter {jitter}"),
            "the edited stroke",
            edited,
            "a fresh blue stroke",
            fresh("Ink Pen", |e| e.set_foreground_color(blue), &ops),
        );
    }
    set_jitter(0.0);
}

/// However many redraws an edit runs, settling it pushes exactly one undo
/// entry, which restores the pre-stroke pixels; redo restores the edit.
#[test]
fn settling_an_edited_stroke_pushes_one_undo_entry() {
    for brush in ["Ink Pen", "Liquify"] {
        let ops = curve();
        let (mut engine, layer) = engine_with("Ink Pen");
        // A ground for liquify to push.
        headless(
            &mut engine,
            layer,
            &[event(40.0, 40.0, 0.0), event(120.0, 90.0, 16.0)],
        );
        engine.commit_pending_edits();
        install_builtin(&mut engine, brush);
        set_input(&mut engine, "brush_settings", "stabilize", 0.0);
        let before = readback(&mut engine, layer);
        let steps = engine.test_undo_len();

        headless(&mut engine, layer, &ops);
        engine.stroke_edit_begin();
        set_input(&mut engine, "brush_settings", "size", 0.4);
        set_input(&mut engine, "brush_settings", "size", 0.3);
        engine.commit_pending_edits();
        assert!(!editing(&engine));
        assert_eq!(
            engine.test_undo_len(),
            steps + 1,
            "the edit must be one undo entry ({brush})"
        );
        let edited = readback(&mut engine, layer);

        engine.undo();
        engine.render(0.0);
        let undone = readback(&mut engine, layer);
        assert!(
            undone.1.contains(before.1),
            "growth is not undone with the pixels ({brush})"
        );
        let grown_before = engine_grown_ground(&before, undone.1);
        assert_eq!(
            undone.0, grown_before,
            "undo must restore the pre-stroke pixels ({brush})"
        );

        engine.redo();
        engine.render(0.0);
        assert_eq!(
            readback(&mut engine, layer).0,
            edited.0,
            "redo must restore the edited stroke ({brush})"
        );
    }
}

/// `before` re-laid onto the grown `bounds`: transparent wherever the layer
/// had no pixels before it grew.
fn engine_grown_ground((pixels, old): &(Vec<u8>, CanvasRect), bounds: CanvasRect) -> Vec<u8> {
    let mut out = vec![0u8; (bounds.width * bounds.height * 4) as usize];
    for y in 0..old.height {
        for x in 0..old.width {
            let src = ((y * old.width + x) * 4) as usize;
            let dx = (old.origin.x - bounds.origin.x) as u32 + x;
            let dy = (old.origin.y - bounds.origin.y) as u32 + y;
            let dst = ((dy * bounds.width + dx) * 4) as usize;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
        }
    }
    out
}

/// On a mask, the undo entry is committed in the mask's one channel: undo
/// restores the mask's pre-stroke bytes.
#[test]
fn mask_stroke_edit_undoes_to_the_pre_stroke_mask() {
    let (mut engine, layer) = engine_with("Ink Pen");
    engine.add_mask(layer).expect("add mask");
    let mask = engine.host_mask_id(layer).expect("mask exists");
    engine.test_flush_readbacks();
    let before = engine.test_readback_layer(mask);
    let steps = engine.test_undo_len();

    // Inside the mask, so it does not grow.
    let ops: Vec<_> = (0..12)
        .map(|i| event(20.0 + i as f32 * 8.0, 64.0, i as f64 * 16.0))
        .collect();
    headless(&mut engine, mask, &ops);
    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.4);
    engine.commit_pending_edits();
    assert_eq!(engine.test_undo_len(), steps + 1);
    engine.test_flush_readbacks();
    assert_ne!(
        engine.test_readback_layer(mask),
        before,
        "the stroke painted the mask"
    );

    engine.undo();
    engine.render(0.0);
    engine.test_flush_readbacks();
    assert_eq!(
        engine.test_readback_layer(mask),
        before,
        "undo must restore the mask's pre-stroke bytes"
    );
}

/// Anything else the artist does ends the edit, and the stroke's entry lands
/// below the next one.
#[test]
fn the_next_action_ends_the_edit() {
    // A new stroke.
    let (mut engine, layer) = engine_with("Ink Pen");
    let steps = engine.test_undo_len();
    headless(&mut engine, layer, &curve());
    engine.stroke_edit_begin();
    engine.begin_stroke(layer).unwrap();
    assert!(!editing(&engine), "a new stroke ends the edit");
    assert_eq!(
        engine.test_undo_len(),
        steps + 1,
        "and settles the edited stroke"
    );
    engine.stroke_to(event(10.0, 10.0, 0.0));
    engine.end_stroke();

    // A document edit that pushes its own entry.
    let (mut engine, layer) = engine_with("Ink Pen");
    let steps = engine.test_undo_len();
    headless(&mut engine, layer, &curve());
    engine.stroke_edit_begin();
    let added = engine.add_raster_layer(None);
    assert!(!editing(&engine));
    assert_eq!(engine.test_undo_len(), steps + 2);
    engine.undo();
    assert!(!engine.has_layer(added), "the layer add is on top");
    assert!(engine.has_layer(layer));

    // A pending-edit session, which resolves edits before pushing anything.
    let (mut engine, layer) = engine_with("Ink Pen");
    let steps = engine.test_undo_len();
    headless(&mut engine, layer, &curve());
    engine.stroke_edit_begin();
    engine.begin_transform(layer);
    assert!(!editing(&engine), "a transform session ends the edit");
    assert_eq!(
        engine.test_undo_len(),
        steps + 1,
        "the stroke settled first"
    );

    // Leaving the edit is not an action: the stroke stays editable.
    let (mut engine, layer) = engine_with("Ink Pen");
    headless(&mut engine, layer, &curve());
    engine.stroke_edit_begin();
    engine.stroke_edit_end();
    assert!(!editing(&engine));
    assert!(engine.stroke_edit_begin(), "the stroke is still kept");
}

/// Undo while editing ends the edit and undoes the stroke.
#[test]
fn undo_while_editing_removes_the_stroke() {
    let (mut engine, layer) = engine_with("Ink Pen");
    let before = readback(&mut engine, layer);
    headless(&mut engine, layer, &curve());
    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.4);

    engine.undo();
    engine.render(0.0);
    assert!(!editing(&engine));
    assert!(engine.test_can_redo());
    let undone = readback(&mut engine, layer);
    assert_eq!(undone.0, engine_grown_ground(&before, undone.1));
}

/// Switching to a brush whose terminal needs a different scratch replaces
/// the scratch, in both directions.
#[test]
fn terminal_change_reallocates_the_scratch() {
    let ops = curve();
    let ground = [event(30.0, 50.0, 0.0), event(110.0, 80.0, 16.0)];
    let paint_ground = |e: &mut DarklyEngine, layer: LayerId| {
        headless(e, layer, &ground);
        e.commit_pending_edits();
        set_input(e, "brush_settings", "stabilize", 0.0);
    };

    let (mut engine, layer) = engine_with("Ink Pen");
    paint_ground(&mut engine, layer);
    headless(&mut engine, layer, &ops);
    let ink = readback(&mut engine, layer);
    engine.stroke_edit_begin();

    install_builtin(&mut engine, "Liquify");
    set_input(&mut engine, "brush_settings", "stabilize", 0.0);
    assert!(editing(&engine), "a brush load keeps the edit");
    let liquify_oracle = {
        let (mut fresh, layer) = engine_with("Ink Pen");
        paint_ground(&mut fresh, layer);
        install_builtin(&mut fresh, "Liquify");
        set_input(&mut fresh, "brush_settings", "stabilize", 0.0);
        headless(&mut fresh, layer, &ops);
        readback(&mut fresh, layer)
    };
    assert_same(
        "Ink Pen edited to Liquify",
        "the edited stroke",
        readback(&mut engine, layer),
        "a fresh Liquify stroke",
        liquify_oracle,
    );

    install_builtin(&mut engine, "Ink Pen");
    set_input(&mut engine, "brush_settings", "stabilize", 0.0);
    assert_same(
        "back to Ink Pen",
        "the edited stroke",
        readback(&mut engine, layer),
        "the original Ink Pen stroke",
        ink,
    );
}

/// Two engines on one device: editing a stroke in one never touches the
/// other.
#[test]
fn an_edit_stays_in_its_own_engine() {
    let (device, queue) = test_device();
    let shared = Arc::new(GpuDevice { device, queue });
    let mut a = DarklyEngine::new(GpuContext::new_headless_shared(Arc::clone(&shared)), 64, 64);
    let mut b = DarklyEngine::new(GpuContext::new_headless_shared(shared), 64, 64);
    let layer_a = a.add_raster_layer(None);
    let layer_b = b.add_raster_layer(None);
    headless(
        &mut a,
        layer_a,
        &[event(10.0, 10.0, 0.0), event(50.0, 40.0, 16.0)],
    );
    headless(
        &mut b,
        layer_b,
        &[event(10.0, 50.0, 0.0), event(50.0, 20.0, 16.0)],
    );
    let b_before = readback(&mut b, layer_b);

    a.stroke_edit_begin();
    a.set_foreground_color([0.9, 0.2, 0.1, 1.0]);
    set_input(&mut a, "brush_settings", "size", 0.4);
    assert!(editing(&a));
    assert!(!editing(&b));
    assert_eq!(readback(&mut b, layer_b), b_before);
}

/// Apply leaves the edit without settling: the stroke stays editable, a
/// second edit redraws it from its events as before, and it still settles
/// as one undo entry.
#[test]
fn apply_keeps_the_stroke_editable() {
    let (mut engine, layer) = engine_with("Ink Pen");
    let before = readback(&mut engine, layer);
    let steps = engine.test_undo_len();
    headless(&mut engine, layer, &curve());

    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.4);
    assert!(engine.apply_edit_session(), "stroke edit is open");
    assert!(!editing(&engine));
    assert_eq!(engine.engine_state().edit_session, None);
    assert_eq!(engine.test_undo_len(), steps, "Apply does not settle");

    assert!(engine.stroke_edit_begin(), "the stroke is still editable");
    set_input(&mut engine, "brush_settings", "size", 0.3);
    assert_same(
        "applied at 0.4, edited again to 0.3",
        "the edited stroke",
        readback(&mut engine, layer),
        "a fresh stroke at 0.3",
        fresh_at(0.3),
    );

    engine.commit_pending_edits();
    assert_eq!(
        engine.test_undo_len(),
        steps + 1,
        "one entry for the stroke"
    );
    engine.undo();
    engine.render(0.0);
    let undone = readback(&mut engine, layer);
    assert_eq!(undone.0, engine_grown_ground(&before, undone.1));
}

/// Discard puts back the stroke as it looked when the edit was entered,
/// before the entry redraw, and leaves the brush and colour alone.
#[test]
fn discard_reverts_to_the_stroke_as_entered() {
    let red = [0.9, 0.2, 0.1, 1.0];
    let (mut engine, layer) = engine_with("Ink Pen");
    set_input(&mut engine, "brush_settings", "size", 0.2);
    headless(&mut engine, layer, &curve());
    let original = readback(&mut engine, layer);

    // Not editing: no redraw.
    set_input(&mut engine, "brush_settings", "size", 0.5);
    engine.stroke_edit_begin();
    assert_same(
        "entered after a size change",
        "the entry redraw",
        readback(&mut engine, layer),
        "a fresh stroke at 0.5",
        fresh_at(0.5),
    );
    set_input(&mut engine, "brush_settings", "size", 0.3);
    engine.set_foreground_color(red);
    assert!(engine.discard_edit_session(), "stroke edit is open");
    assert!(!editing(&engine));
    assert_same(
        "discarded",
        "the discarded stroke",
        readback(&mut engine, layer),
        "the stroke as painted",
        original,
    );

    engine.stroke_edit_begin();
    assert_same(
        "re-entered after a discard",
        "the entry redraw",
        readback(&mut engine, layer),
        "a fresh red stroke at 0.3",
        fresh(
            "Ink Pen",
            |e| {
                set_input(e, "brush_settings", "size", 0.3);
                e.set_foreground_color(red);
            },
            &curve(),
        ),
    );
}

/// Discard after an Apply and a second entry gives back the applied
/// version.
#[test]
fn discard_after_apply_returns_the_applied_version() {
    let (mut engine, layer) = engine_with("Ink Pen");
    headless(&mut engine, layer, &curve());
    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.4);
    engine.apply_edit_session();
    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.15);
    engine.discard_edit_session();
    assert_same(
        "applied at 0.4, then 0.15 discarded",
        "the discarded stroke",
        readback(&mut engine, layer),
        "a fresh stroke at 0.4",
        fresh_at(0.4),
    );
}

/// An edit that grows the stroke past its entry footprint is discarded
/// entirely, and the settled entry still undoes to the pre-stroke pixels
/// and redoes to the stroke as entered.
#[test]
fn discard_restores_beyond_a_grown_footprint() {
    let (mut engine, layer) = engine_with("Ink Pen");
    let before = readback(&mut engine, layer);
    set_input(&mut engine, "brush_settings", "size", 0.1);
    headless(&mut engine, layer, &curve());
    let original = readback(&mut engine, layer);

    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.8);
    assert_ne!(readback(&mut engine, layer).0, original.0);
    engine.discard_edit_session();
    assert_same(
        "0.1 edited to 0.8, discarded",
        "the discarded stroke",
        readback(&mut engine, layer),
        "the stroke as painted",
        original.clone(),
    );

    engine.commit_pending_edits();
    engine.undo();
    engine.render(0.0);
    let undone = readback(&mut engine, layer);
    assert_eq!(undone.0, engine_grown_ground(&before, undone.1));
    engine.redo();
    engine.render(0.0);
    assert_eq!(readback(&mut engine, layer).0, original.0);
}

/// An edit that shrinks the stroke inside its entry footprint: Discard must
/// put the entry footprint back with the pixels, or the settled entry
/// would undo only the shrunk rect and leave the ring around it painted.
#[test]
fn discard_restores_a_shrunk_footprint() {
    let (mut engine, layer) = engine_with("Ink Pen");
    let before = readback(&mut engine, layer);
    set_input(&mut engine, "brush_settings", "size", 0.8);
    headless(&mut engine, layer, &curve());

    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.05);
    engine.discard_edit_session();
    engine.commit_pending_edits();
    engine.undo();
    engine.render(0.0);
    let undone = readback(&mut engine, layer);
    assert_eq!(
        undone.0,
        engine_grown_ground(&before, undone.1),
        "undo must restore the whole entry footprint"
    );
}

/// On a mask, Discard restores the mask's bytes as they were at entry.
#[test]
fn mask_discard_restores_the_mask() {
    let (mut engine, layer) = engine_with("Ink Pen");
    engine.add_mask(layer).expect("add mask");
    let mask = engine.host_mask_id(layer).expect("mask exists");
    // Inside the mask, so it does not grow.
    let ops: Vec<_> = (0..12)
        .map(|i| event(20.0 + i as f32 * 8.0, 64.0, i as f64 * 16.0))
        .collect();
    headless(&mut engine, mask, &ops);
    engine.test_flush_readbacks();
    let painted = engine.test_readback_layer(mask);

    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.6);
    engine.test_flush_readbacks();
    assert_ne!(engine.test_readback_layer(mask), painted);
    engine.discard_edit_session();
    engine.test_flush_readbacks();
    assert_eq!(
        engine.test_readback_layer(mask),
        painted,
        "Discard must restore the mask's bytes"
    );
}

/// Entering while already editing keeps the first revert point.
#[test]
fn entering_twice_keeps_the_first_revert_point() {
    let (mut engine, layer) = engine_with("Ink Pen");
    headless(&mut engine, layer, &curve());
    let original = readback(&mut engine, layer);
    engine.stroke_edit_begin();
    set_input(&mut engine, "brush_settings", "size", 0.4);
    assert!(engine.stroke_edit_begin());
    engine.discard_edit_session();
    assert_same(
        "entered twice, discarded",
        "the discarded stroke",
        readback(&mut engine, layer),
        "the stroke as painted",
        original,
    );
}
