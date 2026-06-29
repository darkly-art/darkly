//! Engine-level GPU integration test for the text / vector layer.
//!
//! Adds a text layer, realizes it through the compositor (Vello render), reads
//! back the layer texture and asserts the glyphs produced coverage, then
//! confirms an undo removes the layer and a redo restores it with text intact.
//!
//! Run with: `cargo test -p darkly --test text_layer --features testing -- --test-threads=1`

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::{TextAlign, TextProps, TextStyle};
use darkly::transform::Transform;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Count non-transparent pixels in an RGBA buffer.
fn covered_pixels(pixels: &[u8]) -> usize {
    pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
}

#[test]
fn text_layer_realizes_glyph_coverage_then_undo_redo() {
    let (w, h) = (256, 128);
    let mut engine = test_engine(w, h);

    let mut text = TextProps::new("Hello".to_string());
    text.size = 64.0;
    let (id, _obj) = engine.add_text_layer(text, 16.0, 16.0, [255, 255, 255, 255], None);
    assert!(engine.has_layer(id), "text layer is in the tree after add");

    // Force a composite so the vector layer realizes via Vello.
    let _ = engine.test_readback_canvas();

    let pixels = engine.test_readback_layer(id);
    let covered = covered_pixels(&pixels);
    assert!(
        covered > 0,
        "rendered text must produce non-empty coverage (got {covered})"
    );

    // Undo removes the layer from the tree.
    engine.undo();
    assert!(!engine.has_layer(id), "undo removes the text layer");

    // Redo restores the layer and it re-realizes (text object survived the
    // undo/redo round-trip — content stability is covered by the layer-kind
    // serialize test).
    engine.redo();
    assert!(engine.has_layer(id), "redo restores the text layer");
    let _ = engine.test_readback_canvas();
    let after = covered_pixels(&engine.test_readback_layer(id));
    assert!(after > 0, "text re-realizes after redo (got {after})");
}

#[test]
fn editing_text_content_re_realizes() {
    let (w, h) = (256, 128);
    let mut engine = test_engine(w, h);

    let (id, obj) = engine.add_text_layer(
        TextProps::new("i".to_string()),
        16.0,
        16.0,
        [255, 255, 255, 255],
        None,
    );
    let _ = engine.test_readback_canvas();
    let thin = covered_pixels(&engine.test_readback_layer(id));

    // A much wider string must cover strictly more pixels after re-realization.
    engine.set_text_content(id, obj, "WWWWWWWW".to_string());
    let _ = engine.test_readback_canvas();
    let wide = covered_pixels(&engine.test_readback_layer(id));

    assert!(
        wide > thin,
        "editing content re-rasterizes: wide ({wide}) should exceed thin ({thin})"
    );
}

#[test]
fn edit_object_by_id_targets_correct_object() {
    let (w, h) = (256, 128);
    let mut engine = test_engine(w, h);

    let (a, obj_a) = engine.add_text_layer(
        TextProps::new("i".into()),
        8.0,
        8.0,
        [255, 255, 255, 255],
        None,
    );
    let (b, obj_b) = engine.add_text_layer(
        TextProps::new("i".into()),
        8.0,
        60.0,
        [255, 255, 255, 255],
        None,
    );
    let _ = engine.test_readback_canvas();
    let a_before = covered_pixels(&engine.test_readback_layer(a));
    let b_before = covered_pixels(&engine.test_readback_layer(b));

    // Editing B's object must not touch A's coverage.
    engine.set_text_content(b, obj_b, "WWWWWWWW".into());
    let _ = engine.test_readback_canvas();
    assert_eq!(
        covered_pixels(&engine.test_readback_layer(a)),
        a_before,
        "editing object B left A's coverage unchanged"
    );
    assert!(
        covered_pixels(&engine.test_readback_layer(b)) > b_before,
        "editing object B grew B's coverage"
    );
    // `obj_a` is addressable independently and unaffected.
    let _ = obj_a;
}

#[test]
fn object_transform_moves_and_coalesces_undo() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);

    let (id, obj) = engine.add_text_layer(
        TextProps::new("lo".into()),
        6.0,
        6.0,
        [255, 255, 255, 255],
        None,
    );
    let _ = engine.test_readback_canvas();
    let base = covered_pixels(&engine.test_readback_layer(id));

    // A whole gizmo drag = several absolute-transform updates; they coalesce
    // into ONE undo step (same object, Transform op).
    for s in [1.3_f32, 1.6, 2.0] {
        engine.set_vector_object_transform(
            id,
            obj,
            Transform::from_affine([s, 0.0, 0.0, 0.0, s, 0.0]),
        );
    }
    let _ = engine.test_readback_canvas();
    let scaled = covered_pixels(&engine.test_readback_layer(id));
    assert!(
        scaled > base,
        "2× scale enlarges coverage ({scaled} > {base})"
    );

    // One undo reverts the whole drag; the layer (the add) survives.
    engine.undo();
    let _ = engine.test_readback_canvas();
    assert_eq!(
        covered_pixels(&engine.test_readback_layer(id)),
        base,
        "a single undo reverts the coalesced transform drag"
    );
    assert!(
        engine.has_layer(id),
        "the add is a separate, still-present step"
    );
    // The next undo removes the layer — proving the transforms were one step.
    engine.undo();
    assert!(!engine.has_layer(id), "second undo removes the layer");
}

#[test]
fn distinct_ops_do_not_over_coalesce() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);

    let (id, obj) = engine.add_text_layer(
        TextProps::new("i".into()),
        6.0,
        6.0,
        [255, 255, 255, 255],
        None,
    );
    let _ = engine.test_readback_canvas();
    let thin = covered_pixels(&engine.test_readback_layer(id));

    // Content edit, then a transform on the SAME object — different ops, so two
    // distinct undo steps despite sharing one `Property::VectorObjects` kind.
    engine.set_text_content(id, obj, "WWWWWWWW".into());
    engine.set_vector_object_transform(
        id,
        obj,
        Transform::from_affine([1.5, 0.0, 0.0, 0.0, 1.5, 0.0]),
    );

    // First undo reverts only the transform — the wide content remains.
    engine.undo();
    let _ = engine.test_readback_canvas();
    let after_one = covered_pixels(&engine.test_readback_layer(id));
    assert!(
        after_one > thin,
        "one undo keeps the content edit ({after_one} > {thin}) — ops did not merge"
    );
    // Second undo reverts the content edit too, back to the placed "i".
    engine.undo();
    let _ = engine.test_readback_canvas();
    assert_eq!(
        covered_pixels(&engine.test_readback_layer(id)),
        thin,
        "two undos fully revert both ops"
    );
}

#[test]
fn text_object_info_round_trips() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);

    let mut text = TextProps::new("Hello".into());
    text.size = 33.0;
    text.weight = 600.0;
    text.style = TextStyle::Italic;
    text.align = TextAlign::Center;
    let (id, obj) = engine.add_text_layer(text, 40.0, 50.0, [10, 20, 30, 255], None);

    let info = engine.text_object_info(id, obj).expect("info");
    assert_eq!(info.content, "Hello");
    assert_eq!(info.size, 33.0);
    assert_eq!(info.weight, 600.0);
    assert!(info.italic);
    assert_eq!(info.align, TextAlign::Center);
    assert_eq!(info.color, [10, 20, 30, 255]);
    assert!((info.ox - 40.0).abs() < 0.01, "placement origin x");
    assert!((info.oy - 50.0).abs() < 0.01, "placement origin y");
    assert!(info.width > 0.0 && info.height > 0.0);
}

#[test]
fn object_id_stable_across_reorder() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);

    let (a, obj_a) = engine.add_text_layer(
        TextProps::new("Ag".into()),
        8.0,
        8.0,
        [255, 255, 255, 255],
        None,
    );
    let (b, _obj_b) = engine.add_text_layer(
        TextProps::new("Ag".into()),
        8.0,
        80.0,
        [255, 255, 255, 255],
        None,
    );

    // Reorder the layers; object identity is independent of tree position.
    engine.move_layer(a, darkly::document::MoveTarget::After(b));
    assert_eq!(
        engine.hit_test_vector_object(a, 12.0, 18.0),
        Some(obj_a),
        "object id is stable across a layer reorder"
    );
}
