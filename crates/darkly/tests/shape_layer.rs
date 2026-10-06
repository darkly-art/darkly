//! Engine-level GPU integration test for parametric shape objects.
//!
//! Places shapes through the engine API (live placement, then commit),
//! realizes them through the compositor (Vello render), and reads the
//! composited canvas back at plane points to assert fill, border, parameter
//! edits, hit testing, undo, pending-edit resolution and crop.
//!
//! Run with: `cargo test -p darkly --test shape_layer --features testing -- --test-threads=1`

use darkly::coord::CanvasRect;
use darkly::engine::protocol::RawParams;
use darkly::engine::shapes::StrokeWire;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::{LayerId, ObjectId};
use darkly::transform::Transform;
use serde_json::json;

const RED: [u8; 4] = [255, 0, 0, 255];

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Begin placing a shape, without committing it.
#[allow(clippy::too_many_arguments)]
fn begin(
    engine: &mut DarklyEngine,
    family: &str,
    params: serde_json::Value,
    (x, y, w, h): (f64, f64, f32, f32),
    fill: Option<[u8; 4]>,
    stroke: Option<StrokeWire>,
) {
    engine.begin_shape(
        family.to_string(),
        RawParams(params),
        x,
        y,
        w,
        h,
        fill,
        stroke,
        None,
    );
}

/// Place a shape (begin, then commit) and return its layer and object.
fn add(
    engine: &mut DarklyEngine,
    family: &str,
    params: serde_json::Value,
    bx: (f64, f64, f32, f32),
    fill: Option<[u8; 4]>,
    stroke: Option<StrokeWire>,
) -> (LayerId, ObjectId) {
    begin(engine, family, params, bx, fill, stroke);
    let r = engine.commit_shape().expect("known family places a shape");
    (r.id, r.object)
}

/// The composited canvas pixel at a plane point.
fn canvas_px(engine: &mut DarklyEngine, plane: (i32, i32)) -> [u8; 4] {
    let canvas = engine.canvas_rect();
    let buf = engine.test_readback_canvas();
    let (x, y) = (plane.0 - canvas.origin.x, plane.1 - canvas.origin.y);
    let i = ((y as u32 * canvas.width + x as u32) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

#[test]
fn rectangle_fills_its_box_and_undo_removes_the_layer() {
    let mut engine = test_engine(128, 128);
    let (id, _) = add(
        &mut engine,
        "rectangle",
        json!({}),
        (20.0, 30.0, 60.0, 40.0),
        Some(RED),
        None,
    );
    assert_eq!(canvas_px(&mut engine, (50, 50)), RED, "inside is filled");
    assert_ne!(canvas_px(&mut engine, (10, 10)), RED, "outside is not");

    engine.undo();
    assert!(!engine.has_layer(id), "undo removes the shape layer");
    assert_ne!(canvas_px(&mut engine, (50, 50)), RED);
    engine.redo();
    assert!(engine.has_layer(id), "redo restores it");
    assert_eq!(canvas_px(&mut engine, (50, 50)), RED, "and it re-realizes");
}

#[test]
fn layer_is_named_after_the_family() {
    let mut engine = test_engine(64, 64);
    let (id, _) = add(
        &mut engine,
        "polygon",
        json!({}),
        (4.0, 4.0, 40.0, 40.0),
        Some(RED),
        None,
    );
    let tree = serde_json::to_value(engine.layer_tree()).unwrap();
    assert!(
        tree.to_string().contains("\"Polygon 1\""),
        "the new layer is named after its family: {tree}"
    );
    assert!(engine.has_layer(id));
}

#[test]
fn unknown_family_or_empty_box_places_nothing() {
    let mut engine = test_engine(64, 64);
    begin(
        &mut engine,
        "not_a_shape",
        json!({}),
        (0.0, 0.0, 10.0, 10.0),
        Some(RED),
        None,
    );
    assert!(engine.commit_shape().is_none());
    begin(
        &mut engine,
        "rectangle",
        json!({}),
        (0.0, 0.0, 0.0, 10.0),
        Some(RED),
        None,
    );
    assert!(engine.commit_shape().is_none());
    assert!(!engine.test_can_undo(), "nothing was recorded");
}

#[test]
fn update_and_commit_without_a_placement_are_noops() {
    let mut engine = test_engine(64, 64);
    engine.update_shape(0.0, 0.0, 10.0, 10.0);
    engine.cancel_shape();
    assert!(engine.commit_shape().is_none());
    assert!(!engine.test_can_undo());
}

#[test]
fn live_placement_renders_before_release() {
    let mut engine = test_engine(128, 128);
    begin(
        &mut engine,
        "rectangle",
        json!({}),
        (10.0, 10.0, 10.0, 10.0),
        Some(RED),
        None,
    );
    assert_ne!(
        canvas_px(&mut engine, (60, 60)),
        RED,
        "outside the small box"
    );
    engine.update_shape(10.0, 10.0, 80.0, 80.0);
    assert_eq!(
        canvas_px(&mut engine, (60, 60)),
        RED,
        "inside the grown box, live"
    );
    assert!(!engine.test_can_undo(), "no undo step while placing");
}

#[test]
fn a_placement_is_one_undo_step_at_its_final_box() {
    let mut engine = test_engine(128, 128);
    begin(
        &mut engine,
        "rectangle",
        json!({}),
        (10.0, 10.0, 10.0, 10.0),
        Some(RED),
        None,
    );
    engine.update_shape(10.0, 10.0, 30.0, 30.0);
    engine.update_shape(20.0, 30.0, 60.0, 40.0);
    let r = engine.commit_shape().expect("placed");
    let (ox, oy, w, h, _) = engine.vector_object_info(r.id, r.object).expect("info");
    assert_eq!((ox, oy, w, h), (0.0, 0.0, 60.0, 40.0));
    assert_eq!(canvas_px(&mut engine, (70, 60)), RED);

    engine.undo();
    assert!(!engine.has_layer(r.id));
    assert!(
        !engine.test_can_undo(),
        "the placement was exactly one step"
    );
    engine.redo();
    assert!(engine.has_layer(r.id));
    assert_eq!(
        canvas_px(&mut engine, (70, 60)),
        RED,
        "redo restores the final box"
    );
}

#[test]
fn a_cancelled_placement_leaves_no_layer_and_no_undo_step() {
    let mut engine = test_engine(128, 128);
    begin(
        &mut engine,
        "ellipse",
        json!({}),
        (10.0, 10.0, 60.0, 60.0),
        Some(RED),
        None,
    );
    let id = engine.shape_placement_layer_for_test().expect("placing");
    assert_eq!(canvas_px(&mut engine, (40, 40)), RED);
    engine.cancel_shape();
    assert!(!engine.test_holds_entity(id), "purged, not merely detached");
    assert!(!engine.test_can_undo());
    assert_ne!(canvas_px(&mut engine, (40, 40)), RED);
}

#[test]
fn undo_mid_placement_commits_then_undoes_it() {
    let mut engine = test_engine(128, 128);
    begin(
        &mut engine,
        "rectangle",
        json!({}),
        (10.0, 10.0, 60.0, 60.0),
        Some(RED),
        None,
    );
    let id = engine.shape_placement_layer_for_test().expect("placing");
    engine.undo();
    assert!(
        !engine.has_layer(id),
        "undo committed the placement, then undid it"
    );
    assert!(engine.test_can_redo(), "the add waits on the redo stack");
    engine.update_shape(0.0, 0.0, 100.0, 100.0);
    assert!(
        engine.commit_shape().is_none(),
        "the release finds no placement"
    );
    engine.redo();
    assert!(engine.has_layer(id));
    assert_eq!(canvas_px(&mut engine, (40, 40)), RED);
    assert_ne!(
        canvas_px(&mut engine, (90, 90)),
        RED,
        "the late update was a no-op"
    );
}

#[test]
fn a_placement_swallowed_by_a_merge_is_not_recorded() {
    let mut engine = test_engine(128, 128);
    let below = engine.add_raster_layer(None);
    begin(
        &mut engine,
        "rectangle",
        json!({}),
        (10.0, 10.0, 60.0, 60.0),
        Some(RED),
        None,
    );
    let id = engine.shape_placement_layer_for_test().expect("placing");
    engine.merge_down(id).expect("merge the placing layer down");
    assert!(engine.commit_shape().is_none(), "the merge adopted it");
    engine.undo();
    assert!(
        engine.has_layer(id),
        "undoing the merge restores the shape layer"
    );
    engine.undo();
    assert!(!engine.has_layer(below));
    assert!(
        !engine.test_can_undo(),
        "the stack held only the raster add and the merge: no add for the shape"
    );
}

#[test]
fn a_placement_swallowed_by_flatten_then_cancelled_undoes_cleanly() {
    let mut engine = test_engine(128, 128);
    begin(
        &mut engine,
        "rectangle",
        json!({}),
        (10.0, 10.0, 60.0, 60.0),
        Some(RED),
        None,
    );
    let id = engine.shape_placement_layer_for_test().expect("placing");
    engine.flatten_image().expect("flatten");
    engine.cancel_shape();
    engine.undo();
    assert!(
        engine.has_layer(id),
        "undoing the flatten restores the placed layer"
    );
    assert_eq!(canvas_px(&mut engine, (40, 40)), RED);
}

#[test]
fn a_zero_width_border_is_no_border() {
    let mut engine = test_engine(64, 64);
    let none = StrokeWire {
        width: 0.0,
        color: RED,
    };
    let (id, obj) = add(
        &mut engine,
        "rectangle",
        json!({}),
        (4.0, 4.0, 40.0, 40.0),
        Some(RED),
        Some(none),
    );
    assert_eq!(engine.shape_objects(id)[0].stroke, None);
    engine.set_shape_style(
        id,
        obj,
        Some(RED),
        Some(StrokeWire {
            width: 3.0,
            color: RED,
        }),
    );
    assert!(engine.shape_objects(id)[0].stroke.is_some());
    engine.set_shape_style(id, obj, Some(RED), Some(none));
    assert_eq!(engine.shape_objects(id)[0].stroke, None);
}

#[test]
fn switching_ellipse_style_retessellates_as_one_step() {
    let mut engine = test_engine(128, 128);
    let (id, obj) = add(
        &mut engine,
        "ellipse",
        json!({}),
        (14.0, 14.0, 100.0, 100.0),
        Some(RED),
        None,
    );
    // Just inside the plain ellipse's top edge: a five-lobe superformula
    // (apex up) leaves the band between two upper lobes empty.
    let probe = (47, 22);
    assert_eq!(
        canvas_px(&mut engine, probe),
        RED,
        "inside the plain ellipse"
    );

    engine.set_shape_params(id, obj, RawParams(json!({ "style": 2 })));
    assert_ne!(
        canvas_px(&mut engine, probe),
        RED,
        "the superformula is drawn"
    );
    let info = &engine.shape_objects(id)[0];
    let style = info.params.iter().find(|p| p.name == "style").unwrap();
    assert_eq!(serde_json::to_value(&style.value).unwrap(), json!(2));
    let m = info.params.iter().find(|p| p.name == "m").unwrap();
    assert_eq!(
        serde_json::to_value(m).unwrap()["visible_when"],
        json!(["style", [2]])
    );

    engine.undo();
    assert_eq!(
        canvas_px(&mut engine, probe),
        RED,
        "one undo restores the ellipse"
    );
    assert!(engine.has_layer(id));
}

#[test]
fn border_without_fill_is_a_ring_and_style_edits_drop_it() {
    let mut engine = test_engine(128, 128);
    let border = StrokeWire {
        width: 6.0,
        color: RED,
    };
    let (id, obj) = add(
        &mut engine,
        "rectangle",
        json!({}),
        (20.0, 20.0, 80.0, 80.0),
        None,
        Some(border),
    );
    assert_eq!(canvas_px(&mut engine, (21, 60)), RED, "edge is stroked");
    assert_ne!(canvas_px(&mut engine, (60, 60)), RED, "centre is clear");

    engine.set_shape_style(id, obj, None, None);
    assert_ne!(canvas_px(&mut engine, (21, 60)), RED, "border removed");

    let info = &engine.shape_objects(id)[0];
    assert_eq!((info.fill, info.stroke), (None, None));
}

#[test]
fn rounding_a_square_clears_its_corner() {
    let mut engine = test_engine(128, 128);
    let (id, obj) = add(
        &mut engine,
        "rectangle",
        json!({ "rounding": 0.0 }),
        (20.0, 20.0, 80.0, 80.0),
        Some(RED),
        None,
    );
    assert_eq!(
        canvas_px(&mut engine, (22, 22)),
        RED,
        "sharp corner is filled"
    );

    engine.set_shape_params(id, obj, RawParams(json!({ "rounding": 1.0 })));
    assert_ne!(
        canvas_px(&mut engine, (22, 22)),
        RED,
        "rounded corner is clear"
    );

    let info = &engine.shape_objects(id)[0];
    assert_eq!(info.family, "rectangle");
    assert_eq!(
        serde_json::to_value(&info.params[0].value).unwrap(),
        json!(1.0)
    );

    engine.undo();
    assert_eq!(
        canvas_px(&mut engine, (22, 22)),
        RED,
        "undo restores the sharp corner"
    );
}

#[test]
fn hit_testing_and_gizmo_frame_use_the_box() {
    let mut engine = test_engine(128, 128);
    let (id, obj) = add(
        &mut engine,
        "ellipse",
        json!({}),
        (20.0, 30.0, 60.0, 40.0),
        Some(RED),
        None,
    );
    assert_eq!(engine.hit_test_vector_object(id, 50.0, 50.0), Some(obj));
    assert_eq!(engine.hit_test_vector_object(id, 10.0, 10.0), None);

    let (ox, oy, w, h, _) = engine.vector_object_info(id, obj).expect("info");
    assert_eq!((ox, oy, w, h), (0.0, 0.0, 60.0, 40.0));

    // Rotate a quarter turn about the box centre (50, 50): the 60 x 40 box
    // becomes 40 x 60, so its old corner region (22, 32) falls outside.
    let rot = kurbo::Affine::translate((50.0, 50.0))
        * kurbo::Affine::rotate(std::f64::consts::FRAC_PI_2)
        * kurbo::Affine::translate((-30.0, -20.0));
    let [a, b, c, d, e, f] = rot.as_coeffs();
    engine.set_vector_object_transform(
        id,
        obj,
        Transform::from_affine([a as f32, c as f32, e as f32, b as f32, d as f32, f as f32]),
    );
    assert_eq!(engine.hit_test_vector_object(id, 50.0, 50.0), Some(obj));
    assert_eq!(engine.hit_test_vector_object(id, 22.0, 32.0), None);
    assert_eq!(engine.hit_test_vector_object(id, 50.0, 78.0), Some(obj));
}

#[test]
fn placement_is_in_plane_space_after_a_crop() {
    let mut engine = test_engine(128, 128);
    engine.resize_canvas(CanvasRect::from_xywh(10, 20, 100, 100));
    let (_, _) = add(
        &mut engine,
        "rectangle",
        json!({}),
        (40.0, 50.0, 20.0, 20.0),
        Some(RED),
        None,
    );
    assert_eq!(
        canvas_px(&mut engine, (50, 60)),
        RED,
        "inside, in plane terms"
    );
    assert_ne!(
        canvas_px(&mut engine, (35, 45)),
        RED,
        "outside, in plane terms"
    );
}
