//! Engine-level GPU integration test for the text / vector layer.
//!
//! Adds a text layer, realizes it through the compositor (Vello render), reads
//! back the layer texture and asserts the glyphs produced coverage, then
//! confirms an undo removes the layer and a redo restores it with text intact.
//!
//! Run with: `cargo test -p darkly --test text_layer --features testing -- --test-threads=1`

use darkly::engine::DarklyEngine;
use darkly::engine::SavePurpose;
use darkly::format::manifest::Manifest;
use darkly::gpu::context::GpuContext;
use darkly::gpu::ortho_transform::OrthoXform;
use darkly::gpu::test_utils::test_device;
use darkly::layer::{TextAlign, TextLayout, TextProps, TextStyle};
use darkly::transform::Transform;

/// A genuine variable font (Cantarell-VF, CFF2, `wght` 100-800 axis) used as a
/// non-fallback fixture: registering it proves upload, and embedding it proves
/// the `.darkly` round-trip. OFL 1.1, © the Cantarell Authors.
const CANTARELL_VF: &[u8] = include_bytes!("fixtures/fonts/Cantarell-VF.otf");

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Drive an in-flight save to completion and hand back the bundle. Mirrors the
/// engine's own save-flow tests: pump readbacks + render until `poll_save_result`
/// yields.
fn drive_save_to_completion(engine: &mut DarklyEngine) -> darkly::format::manifest::SaveBundle {
    engine
        .start_save_document(SavePurpose::File)
        .expect("save kicks off");
    for _ in 0..32 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if let Some(bundle) = engine.poll_save_result() {
            return bundle;
        }
    }
    panic!("save did not complete within 32 frames");
}

/// Count non-transparent pixels in an RGBA buffer.
fn covered_pixels(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] > 0)
        .count()
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
    // undo/redo round-trip; content stability is covered by the layer-kind
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
    // The next undo removes the layer, proving the transforms were one step.
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

    // Content edit, then a transform on the SAME object: different ops, so two
    // distinct undo steps despite sharing one `Property::VectorObjects` kind.
    engine.set_text_content(id, obj, "WWWWWWWW".into());
    engine.set_vector_object_transform(
        id,
        obj,
        Transform::from_affine([1.5, 0.0, 0.0, 0.0, 1.5, 0.0]),
    );

    // First undo reverts only the transform; the wide content remains.
    engine.undo();
    let _ = engine.test_readback_canvas();
    let after_one = covered_pixels(&engine.test_readback_layer(id));
    assert!(
        after_one > thin,
        "one undo keeps the content edit ({after_one} > {thin}); ops did not merge"
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
fn text_objects_lists_objects() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);

    let mut text = TextProps::new("Hello".into());
    text.size = 33.0;
    text.variations.insert("wght".to_string(), 600.0);
    text.style = TextStyle::Italic;
    text.align = TextAlign::Center;
    let (id, obj) = engine.add_text_layer(text, 40.0, 50.0, [10, 20, 30, 255], None);

    let objects = engine.text_objects(id);
    assert_eq!(objects.len(), 1, "one text object on the new layer");
    let o = &objects[0];
    assert_eq!(o.object, obj);
    assert_eq!(o.content, "Hello");
    assert_eq!(o.size, 33.0);
    assert_eq!(o.variations.get("wght"), Some(&600.0));
    assert!(o.italic);
    assert_eq!(o.align, TextAlign::Center);
    assert_eq!(o.color, [10, 20, 30, 255]);
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
    engine
        .move_layer(a, darkly::document::MoveTarget::After(b))
        .expect("move succeeds");
    assert_eq!(
        engine.hit_test_vector_object(a, 12.0, 18.0),
        Some(obj_a),
        "object id is stable across a layer reorder"
    );
}

#[test]
fn area_text_bbox_is_the_box_not_the_natural_width() {
    let mut engine = test_engine(256, 256);
    let mut text = TextProps::new("the quick brown fox jumps".to_string());
    text.size = 24.0;
    text.layout = TextLayout::Area {
        width: 120.0,
        height: 80.0,
    };
    let (id, obj) = engine.add_text_layer(text, 10.0, 10.0, [255, 255, 255, 255], None);

    let (_ox, _oy, w, h, _m) = engine
        .vector_object_info(id, obj)
        .expect("vector_object_info for the area-text object");
    assert!(
        (w - 120.0).abs() < 0.5,
        "the gizmo bbox width is the box width (got {w})"
    );
    assert!(
        (h - 80.0).abs() < 0.5,
        "the gizmo bbox height is the box height (got {h})"
    );
}

#[test]
fn set_text_box_converts_point_text_and_coalesces_undo() {
    let mut engine = test_engine(256, 256);
    let (id, obj) = engine.add_text_layer(
        TextProps::new("hello world".into()),
        10.0,
        10.0,
        [255, 255, 255, 255],
        None,
    );
    // Born as point text, no box.
    assert_eq!(
        engine.text_objects(id)[0].box_size,
        None,
        "point text has no box"
    );

    // A resize drag is several set_text_box calls; they coalesce to one step.
    let g = Transform::from_affine([1.0, 0.0, 10.0, 0.0, 1.0, 10.0]);
    engine.set_text_box(id, obj, g, (100.0, 60.0));
    engine.set_text_box(id, obj, g, (140.0, 90.0));
    assert_eq!(
        engine.text_objects(id)[0].box_size,
        Some((140.0, 90.0)),
        "the resize set the box to its final size"
    );

    // One undo reverts the whole drag back to point text...
    engine.undo();
    assert_eq!(
        engine.text_objects(id)[0].box_size,
        None,
        "one undo reverts the coalesced resize drag to point text"
    );
    // ...and the next undo removes the layer (the original add).
    engine.undo();
    assert!(!engine.has_layer(id), "second undo removes the text layer");
}

#[test]
fn add_text_object_appends_to_existing_layer_as_one_undo_step() {
    let mut engine = test_engine(256, 256);
    // A vector layer with one text object.
    let (id, obj_a) = engine.add_text_layer(
        TextProps::new("first".into()),
        10.0,
        10.0,
        [255, 255, 255, 255],
        None,
    );
    assert_eq!(engine.text_objects(id).len(), 1);

    // A second text box lands on the SAME layer (the multi-object case).
    let obj_b = engine
        .add_text_object(
            id,
            TextProps::new("second".into()),
            10.0,
            80.0,
            [255, 255, 255, 255],
        )
        .expect("add_text_object on a vector layer");
    assert_ne!(obj_a, obj_b, "the new object gets a distinct id");
    assert_eq!(
        engine.text_objects(id).len(),
        2,
        "both text objects live on one layer"
    );

    // One undo removes just the added object; the layer and first object remain.
    engine.undo();
    assert!(engine.has_layer(id), "undo keeps the layer");
    let objs = engine.text_objects(id);
    assert_eq!(objs.len(), 1, "undo removes only the appended object");
    assert_eq!(objs[0].object, obj_a, "the original object is untouched");
}

#[test]
fn add_text_object_rejects_a_non_vector_layer() {
    let mut engine = test_engine(128, 128);
    let raster = engine.add_raster_layer(None);
    assert!(
        engine
            .add_text_object(raster, TextProps::new("x".into()), 0.0, 0.0, [0, 0, 0, 255])
            .is_none(),
        "add_text_object is a no-op on a raster layer"
    );
}

/// Registering a real second font's bytes grows the picker list and makes the
/// family resolvable for shaping: the shared contract all three ingestion
/// paths (upload, Google, embedded) converge on.
#[test]
fn register_font_adds_family() {
    let mut engine = test_engine(256, 128);
    let before = engine.list_fonts().len();

    let families = engine.register_font(CANTARELL_VF.to_vec());
    let family = families
        .first()
        .expect("Cantarell-VF contributes a family")
        .clone();
    assert!(
        engine.list_fonts().contains(&family),
        "the registered family joins the picker list"
    );
    assert!(
        engine.list_fonts().len() > before,
        "the family list grew by the registration"
    );

    // The family shapes to real coverage (not an empty fallback miss).
    let mut text = TextProps::new("Hello".to_string());
    text.font_family = family;
    text.size = 64.0;
    let (id, _obj) = engine.add_text_layer(text, 16.0, 16.0, [255, 255, 255, 255], None);
    let _ = engine.test_readback_canvas();
    assert!(
        covered_pixels(&engine.test_readback_layer(id)) > 0,
        "text in the registered family produces coverage"
    );
}

/// Dedup regression: two text objects sharing one font family must emit a
/// **single** `fonts/*` blob and one `manifest.fonts` entry, not one per
/// object. Fails against a naive per-object embed.
#[test]
fn same_font_embedded_once() {
    let mut engine = test_engine(128, 64);
    let family = engine
        .register_font(CANTARELL_VF.to_vec())
        .first()
        .expect("family")
        .clone();

    let mut a = TextProps::new("A".to_string());
    a.font_family = family.clone();
    a.size = 24.0;
    let (layer, _obj_a) = engine.add_text_layer(a, 4.0, 4.0, [255, 255, 255, 255], None);

    let mut b = TextProps::new("B".to_string());
    b.font_family = family.clone();
    b.size = 24.0;
    engine
        .add_text_object(layer, b, 60.0, 4.0, [255, 255, 255, 255])
        .expect("second text object on the same layer");

    let bundle = drive_save_to_completion(&mut engine);
    let manifest: Manifest = serde_json::from_slice(&bundle.manifest_json).unwrap();

    assert_eq!(
        manifest.fonts.len(),
        1,
        "one manifest.fonts entry for the shared family (got {:?})",
        manifest.fonts
    );
    let font_blobs = bundle
        .blobs
        .iter()
        .filter(|blk| blk.path.starts_with("fonts/"))
        .count();
    assert_eq!(
        font_blobs, 1,
        "exactly one fonts/* blob for the shared bytes"
    );
}

/// Plane-space bounding box `(x0, y0, x1, y1)` (exclusive max) of every
/// composited pixel with any coverage, or `None` for an empty canvas.
fn ink_bbox(engine: &mut DarklyEngine) -> Option<(i32, i32, i32, i32)> {
    let canvas = engine.canvas_rect();
    let pixels = engine.test_readback_canvas();
    let mut bbox: Option<(i32, i32, i32, i32)> = None;
    for (i, p) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if p[3] == 0 {
            continue;
        }
        let x = canvas.origin.x + (i as u32 % canvas.width) as i32;
        let y = canvas.origin.y + (i as u32 / canvas.width) as i32;
        bbox = Some(match bbox {
            None => (x, y, x + 1, y + 1),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
        });
    }
    bbox
}

/// Whether `inner` lies inside `outer` grown by `slack` on every side.
fn within(inner: (i32, i32, i32, i32), outer: (i32, i32, i32, i32), slack: i32) -> bool {
    inner.0 >= outer.0 - slack
        && inner.1 >= outer.1 - slack
        && inner.2 <= outer.2 + slack
        && inner.3 <= outer.3 + slack
}

fn set_text_color(
    engine: &mut DarklyEngine,
    id: darkly::layer::LayerId,
    obj: darkly::layer::ObjectId,
    color: [u8; 4],
) {
    engine.set_text_style(
        id,
        obj,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(color),
    );
}

/// Text added after a crop is drawn where the document places it: its ink
/// sits inside the object's plane box, and that is where it hit-tests.
#[test]
fn text_is_placed_in_plane_space_after_a_crop() {
    let mut engine = test_engine(256, 128);
    engine.resize_canvas(darkly::coord::CanvasRect::from_xywh(60, 40, 180, 80));
    let mut text = TextProps::new("W".to_string());
    text.size = 48.0;
    let (id, obj) = engine.add_text_layer(text, 70.0, 45.0, [255, 255, 255, 255], None);

    let (_, _, w, h, g) = engine.vector_object_info(id, obj).expect("info");
    let corner = |x: f32, y: f32| darkly::transform::affine_transform(&g, x, y);
    let (bx0, by0) = corner(0.0, 0.0);
    let (bx1, by1) = corner(w, h);
    let object_box = (bx0 as i32, by0 as i32, bx1.ceil() as i32, by1.ceil() as i32);

    let ink = ink_bbox(&mut engine).expect("the text draws something");
    assert!(
        within(ink, object_box, 4),
        "ink {ink:?} lies outside the object's plane box {object_box:?}"
    );
    let (cx, cy) = ((ink.0 + ink.2) as f64 / 2.0, (ink.1 + ink.3) as f64 / 2.0);
    assert_eq!(engine.hit_test_vector_object(id, cx, cy), Some(obj));
}

/// Rotating the canvas a quarter turn carries the text with it. On a
/// 128 x 64 canvas the window recentres to (32, -32, 64, 128) and
/// window-local (x, y) maps to (64 - y, x), so plane (x, y) lands at
/// (96 - y, x - 32). The old window maps exactly onto the new one, so the
/// visible ink maps onto the visible ink.
#[test]
fn rotating_the_canvas_rotates_text() {
    let mut engine = test_engine(128, 64);
    let mut text = TextProps::new("W".to_string());
    text.size = 32.0;
    let (id, obj) = engine.add_text_layer(text, 16.0, 8.0, [255, 255, 255, 255], None);
    let (x0, y0, x1, y1) = ink_bbox(&mut engine).expect("the text draws something");

    engine.transform_canvas(OrthoXform::Rot90Cw);
    let expected = (96 - y1, x0 - 32, 96 - y0, x1 - 32);
    let ink = ink_bbox(&mut engine).expect("the rotated text draws something");
    assert!(
        within(ink, expected, 2) && within(expected, ink, 2),
        "rotated ink {ink:?}, expected about {expected:?}"
    );
    let (cx, cy) = ((ink.0 + ink.2) as f64 / 2.0, (ink.1 + ink.3) as f64 / 2.0);
    assert_eq!(engine.hit_test_vector_object(id, cx, cy), Some(obj));
}

/// Flipping a text layer mirrors its ink about the canvas, and the flip
/// survives re-rendering the layer (a colour change) and undoes cleanly.
#[test]
fn flipping_a_text_layer_mirrors_its_ink() {
    let mut engine = test_engine(128, 64);
    let mut text = TextProps::new("L".to_string());
    text.size = 40.0;
    let (id, obj) = engine.add_text_layer(text, 12.0, 8.0, [255, 255, 255, 255], None);
    let before = ink_bbox(&mut engine).expect("the text draws something");
    let mirrored = (128 - before.2, before.1, 128 - before.0, before.3);

    assert!(engine.flip_node(id, OrthoXform::FlipH));
    let ink = ink_bbox(&mut engine).expect("flipped text draws something");
    assert!(
        within(ink, mirrored, 2) && within(mirrored, ink, 2),
        "flipped ink {ink:?}, expected about {mirrored:?}"
    );

    set_text_color(&mut engine, id, obj, [255, 0, 0, 255]);
    let ink = ink_bbox(&mut engine).expect("recoloured text draws something");
    assert!(
        within(ink, mirrored, 2) && within(mirrored, ink, 2),
        "after re-render the ink {ink:?} is no longer mirrored ({mirrored:?})"
    );

    engine.undo();
    engine.undo();
    let ink = ink_bbox(&mut engine).expect("text draws something after undo");
    assert!(
        within(ink, before, 2) && within(before, ink, 2),
        "after undo the ink {ink:?} is not back at {before:?}"
    );
}
