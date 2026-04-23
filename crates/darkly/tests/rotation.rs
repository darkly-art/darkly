//! Tests for the angle-wire convention: `pen_input.drawing_angle` (radians)
//! should flow into `stamp.rotation` (also radians, post-fix) and produce
//! a brush tip that faces the direction of travel.
//!
//! The stamp shader divides UVs per-axis by the viewport size, so a
//! non-square tip breaks under rotation — it must be sampled through a
//! matching-aspect viewport. We therefore use a **square** 32×32 tip with
//! a thin horizontal bright bar across the middle rows. Rotation orients
//! that bar:
//! - rotation = 0 → bar horizontal → painted bbox wider than tall
//! - rotation = π/2 → bar vertical → painted bbox taller than wide
//!
//! Under the pre-fix buggy convention (`rotation_input * TAU`), an input
//! of π/2 gives `π² ≈ 9.87 rad ≈ 205°`, which is close to a 180° flip —
//! the bar stays roughly horizontal — so the test fails.

use darkly::brush::wire::BrushWireType;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::params::ParamValue;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{NodeId, NodeInstance};

const CANVAS: u32 = 256;

fn test_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, CANVAS, CANVAS)
}

fn find_node_id(engine: &DarklyEngine, type_id: &str) -> NodeId {
    engine
        .active_brush_graph_ref()
        .nodes
        .values()
        .find(|n: &&NodeInstance<BrushWireType>| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in graph"))
        .id
}

/// Replace the default `circle → stamp.tip` wire with a horizontal-bar
/// `image → stamp.tip` wire. Returns (stamp_id, image_id).
///
/// Upload order matters: `compile_active` prunes any static texture whose
/// resource name isn't referenced by a live Image node. So we build the
/// graph (with the image node's `resource_name` pointing at `test-bar`)
/// *before* uploading the pixels, otherwise the upload gets GC'd on the
/// next graph-mutating call.
fn install_bar_tip(engine: &mut DarklyEngine) -> (NodeId, u64) {
    let stamp_id = find_node_id(engine, "stamp");
    let circle_id = find_node_id(engine, "circle");

    // Add the image node first so its `resource_name` param can be set to
    // "test-bar" before any upload happens.
    let before_ids: std::collections::HashSet<u64> = engine
        .active_brush_graph_ref()
        .nodes
        .keys()
        .map(|id| id.0)
        .collect();
    engine
        .brush_graph_add_node("image", 100.0, 600.0)
        .expect("add image node");
    let image_id = engine
        .active_brush_graph_ref()
        .nodes
        .keys()
        .map(|id| id.0)
        .find(|id| !before_ids.contains(id))
        .expect("new image node id");
    engine
        .brush_graph_set_param(image_id, 0, ParamValue::String("test-bar".into()))
        .unwrap();

    engine
        .brush_graph_disconnect(circle_id.0, "texture", stamp_id.0, "tip")
        .unwrap();
    engine
        .brush_graph_connect(image_id, "texture", stamp_id.0, "tip")
        .unwrap();

    // Default graph wires `pen.pressure → stamp.size`; disconnect it so
    // set_port_default for `size` takes effect (a wire would dominate the
    // default). The shader's per-axis UV division also needs a square
    // viewport for rotation to behave — which the wire's pressure=1
    // blows up to a 512px dab that clips the canvas regardless of tip
    // aspect.
    let _ = engine.brush_graph_disconnect(
        find_node_id(engine, "pen_input").0,
        "pressure",
        stamp_id.0,
        "size",
    );

    // Upload last — the graph now references "test-bar", so the upload
    // survives subsequent `compile_active` sweeps.
    //
    // Square 32×32 tip with a thin horizontal bright bar centered on
    // rows 13..18. The shader's per-axis UV division requires a square
    // viewport for rotation to produce a matching-aspect rotated result.
    const N: u32 = 32;
    const BAR_Y0: u32 = 13;
    const BAR_Y1: u32 = 19;
    let mut pixels = vec![0u8; (N * N * 4) as usize];
    for y in BAR_Y0..BAR_Y1 {
        for x in 0..N {
            let idx = ((y * N + x) * 4) as usize;
            pixels[idx] = 255;
            pixels[idx + 1] = 255;
            pixels[idx + 2] = 255;
            pixels[idx + 3] = 255;
        }
    }
    engine.brush_upload_image("test-bar", N, N, &pixels).unwrap();

    (stamp_id, image_id)
}

/// Canvas-pixel bounding box of non-transparent pixels within an AABB window.
fn painted_bbox(
    pixels: &[u8],
    w: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = ((y * w + x) * 4 + 3) as usize;
            if pixels[idx] > 16 {
                found = true;
                if x < min_x {
                    min_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if x > max_x {
                    max_x = x;
                }
                if y > max_y {
                    max_y = y;
                }
            }
        }
    }
    if found {
        Some((min_x, min_y, max_x, max_y))
    } else {
        None
    }
}

fn stroke_event(x: f32, y: f32, time_ms: f64) -> StrokeOp {
    StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms,
        cr: 1.0,
        cg: 1.0,
        cb: 1.0,
        ca: 1.0,
    }
}

/// Paint with stamp.rotation pinned to a radian value. Confirms the wire
/// contract: the value set as the port default ends up as the rotation
/// angle fed to the stamp shader.
///
/// At rotation=0 the bar tip is horizontal; at rotation=π/2 it is vertical.
#[test]
fn stamp_rotation_port_default_is_radians() {
    // Horizontal tip (rotation = 0).
    let mut engine = test_engine();
    let layer_id = engine.add_raster_layer();
    let (stamp_id, _image_id) = install_bar_tip(&mut engine);

    // Big enough to see the orientation; set size/scale to make dab ~128px.
    engine
        .brush_graph_set_port_default(stamp_id.0, "size", 0.25)
        .unwrap();
    engine
        .brush_graph_set_port_default(stamp_id.0, "scale", 1.0)
        .unwrap();
    engine
        .brush_graph_set_port_default(stamp_id.0, "rotation", 0.0)
        .unwrap();
    // Disable stabilization so the single-event stroke renders immediately.
    let pen_id = find_node_id(&engine, "pen_input");
    engine
        .brush_graph_set_port_default(pen_id.0, "stabilize", 0.0)
        .unwrap();

    engine.begin_stroke(layer_id);
    engine.stroke_to(stroke_event(128.0, 128.0, 0.0));
    engine.end_stroke();
    engine.render(0.0);

    let pixels_h = engine.test_readback_layer(layer_id);
    let bbox_h = painted_bbox(&pixels_h, CANVAS, 0, 0, CANVAS, CANVAS)
        .expect("horizontal tip should paint some pixels");
    let (hx0, hy0, hx1, hy1) = bbox_h;
    let h_w = hx1 - hx0;
    let h_h = hy1 - hy0;
    assert!(
        h_w > h_h * 2,
        "rotation=0 with 4:1 bar tip: expected wide bbox, got {h_w}x{h_h}"
    );

    // Rotated by π/2 (quarter turn clockwise).
    let mut engine = test_engine();
    let layer_id = engine.add_raster_layer();
    let (stamp_id, _) = install_bar_tip(&mut engine);
    engine
        .brush_graph_set_port_default(stamp_id.0, "size", 0.25)
        .unwrap();
    engine
        .brush_graph_set_port_default(stamp_id.0, "scale", 1.0)
        .unwrap();
    engine
        .brush_graph_set_port_default(stamp_id.0, "rotation", std::f32::consts::FRAC_PI_2)
        .unwrap();
    let pen_id = find_node_id(&engine, "pen_input");
    engine
        .brush_graph_set_port_default(pen_id.0, "stabilize", 0.0)
        .unwrap();

    engine.begin_stroke(layer_id);
    engine.stroke_to(stroke_event(128.0, 128.0, 0.0));
    engine.end_stroke();
    engine.render(0.0);

    let pixels_v = engine.test_readback_layer(layer_id);
    let bbox_v = painted_bbox(&pixels_v, CANVAS, 0, 0, CANVAS, CANVAS)
        .expect("vertical tip should paint some pixels");
    let (vx0, vy0, vx1, vy1) = bbox_v;
    let v_w = vx1 - vx0;
    let v_h = vy1 - vy0;
    assert!(
        v_h > v_w * 2,
        "rotation=π/2 with 4:1 bar tip: expected tall bbox, got {v_w}x{v_h} \
         (under buggy *TAU convention this would rotate by ~205° and stay \
         roughly horizontal)"
    );
}

/// Wire `pen_input.drawing_angle → stamp.rotation` and draw a short
/// downward stroke. The bar tip should orient vertically at the end of
/// the stroke, proving the sensor → port wire unit contract works.
#[test]
fn drawing_angle_wire_rotates_brush_to_face_stroke() {
    let mut engine = test_engine();
    let layer_id = engine.add_raster_layer();
    let (stamp_id, _image_id) = install_bar_tip(&mut engine);

    engine
        .brush_graph_set_port_default(stamp_id.0, "size", 0.2)
        .unwrap();
    engine
        .brush_graph_set_port_default(stamp_id.0, "scale", 1.0)
        .unwrap();

    let pen_id = find_node_id(&engine, "pen_input");
    // Disable stabilization so drawing_angle computed from raw events is
    // what lands in the shader on the second dab (no smoothing lag).
    engine
        .brush_graph_set_port_default(pen_id.0, "stabilize", 0.0)
        .unwrap();
    // Tight spacing so multiple dabs drop along a short segment.
    engine
        .brush_graph_set_port_default(pen_id.0, "spacing", 0.05)
        .unwrap();

    engine
        .brush_graph_connect(pen_id.0, "drawing_angle", stamp_id.0, "rotation")
        .unwrap();

    // Downward stroke: `drawing_angle = atan2(dy, dx) = π/2` for pure
    // downward motion. The first two dabs still lerp angle 0→π/2 as the
    // stroke engine transitions from the directionless stroke-start to a
    // known direction, so we scan only pixels well below the stroke head
    // where every dab was drawn with a stable π/2.
    engine.begin_stroke(layer_id);
    engine.stroke_to(stroke_event(128.0, 20.0, 0.0));
    for i in 1..=5 {
        engine.stroke_to(stroke_event(128.0, 20.0 + i as f32 * 40.0, i as f64 * 16.0));
    }
    engine.end_stroke();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer_id);
    // Scan pixels below y=150, where every dab was placed with a stable
    // downward direction (lerp between consecutive π/2 endpoints stays π/2).
    let bbox = painted_bbox(&pixels, CANVAS, 0, 150, CANVAS, CANVAS)
        .expect("downward stroke should paint pixels below y=150");
    let (x0, y0, x1, y1) = bbox;
    let w = x1 - x0;
    let h = y1 - y0;
    assert!(
        h > w * 2,
        "downward stroke with drawing_angle → rotation wire: expected \
         bbox taller than wide, got {w}x{h} (bbox {x0},{y0}-{x1},{y1}). \
         Buggy convention would rotate by π/2 × TAU ≈ 205°, leaving the \
         tip roughly horizontal."
    );
}
