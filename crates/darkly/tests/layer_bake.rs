//! Integration tests for Duplicate / Merge Down / Flatten Image.
//!
//! Run with: `cargo test -p darkly --test layer_bake -- --test-threads=1`

use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Paint a solid coloured stamp at canvas centre. Used to give layers
/// distinguishable pixel content before merge/flatten.
fn paint_dot(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32, color: [f32; 3]) {
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: color[0],
        cg: color[1],
        cb: color[2],
        ca: 1.0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

fn alpha_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[((y * w + x) * 4 + 3) as usize]
}

// ============================================================================
// Duplicate
// ============================================================================

#[test]
fn duplicate_raster_copies_pixels() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_a = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer_a, 64.0, 64.0, [1.0, 0.0, 0.0]);

    let layer_b = engine
        .duplicate_node(layer_a)
        .expect("duplicate returns id");
    assert_ne!(layer_a, layer_b, "duplicate must mint a fresh id");

    let pixels_a = engine.test_readback_layer(layer_a);
    let pixels_b = engine.test_readback_layer(layer_b);
    assert_eq!(
        pixels_a, pixels_b,
        "duplicate layer pixels must match source byte-for-byte"
    );
}

#[test]
fn duplicate_undo_removes_then_redo_restores() {
    let (w, h) = (96, 96);
    let mut engine = test_engine(w, h);
    let layer_a = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer_a, 48.0, 48.0, [0.0, 1.0, 0.0]);

    let layer_b = engine.duplicate_node(layer_a).expect("duplicate succeeded");
    assert!(engine.has_layer(layer_b), "dup attached after creation");

    engine.undo();
    assert!(!engine.has_layer(layer_b), "dup detached after undo");
    assert!(engine.has_layer(layer_a), "source untouched by undo");

    engine.redo();
    assert!(engine.has_layer(layer_b), "dup reattached after redo");

    // After redo the dup's texture should still match the source.
    let pixels_a = engine.test_readback_layer(layer_a);
    let pixels_b = engine.test_readback_layer(layer_b);
    assert_eq!(pixels_a, pixels_b, "redo restores dup pixels");
}

// ============================================================================
// Merge Down
// ============================================================================

#[test]
fn merge_down_baked_result_combines_two_layers() {
    // Two layers each with a different-colour dot. Merging should leave a
    // single raster with both dots present.
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let lower = engine.add_raster_layer(None);
    paint_dot(&mut engine, lower, 32.0, 64.0, [1.0, 0.0, 0.0]); // red dot on lower
    let upper = engine.add_raster_layer(None);
    paint_dot(&mut engine, upper, 96.0, 64.0, [0.0, 0.0, 1.0]); // blue dot on upper

    // Active = upper; merge down folds upper + lower into one raster.
    let result = engine.merge_down(upper).expect("merge_down should succeed");
    assert_ne!(result, lower);
    assert_ne!(result, upper);
    assert!(engine.has_layer(result));
    assert!(!engine.has_layer(lower), "lower consumed by merge");
    assert!(!engine.has_layer(upper), "upper consumed by merge");

    // Both dots should be visible in the result.
    let pixels = engine.test_readback_layer(result);
    assert!(
        alpha_at(&pixels, w, 32, 64) > 0,
        "left dot from lower should be in the result"
    );
    assert!(
        alpha_at(&pixels, w, 96, 64) > 0,
        "right dot from upper should be in the result"
    );
}

#[test]
fn merge_down_fails_on_bottom_layer() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let only = engine.add_raster_layer(None);
    let result = engine.merge_down(only);
    assert!(result.is_err(), "no sibling below → must error");
}

#[test]
fn merge_down_undo_restores_both_sources() {
    let (w, h) = (96, 96);
    let mut engine = test_engine(w, h);
    let lower = engine.add_raster_layer(None);
    paint_dot(&mut engine, lower, 32.0, 48.0, [1.0, 0.0, 0.0]);
    let upper = engine.add_raster_layer(None);
    paint_dot(&mut engine, upper, 64.0, 48.0, [0.0, 0.0, 1.0]);

    let result = engine.merge_down(upper).expect("merge succeeded");

    engine.undo();
    assert!(engine.has_layer(lower), "lower restored");
    assert!(engine.has_layer(upper), "upper restored");
    assert!(!engine.has_layer(result), "result detached on undo");

    // Source pixels must be intact — tombstoning kept textures alive.
    let lower_px = engine.test_readback_layer(lower);
    assert!(
        alpha_at(&lower_px, w, 32, 48) > 0,
        "lower's pixels survive undo"
    );
    let upper_px = engine.test_readback_layer(upper);
    assert!(
        alpha_at(&upper_px, w, 64, 48) > 0,
        "upper's pixels survive undo"
    );
}

// ============================================================================
// Flatten Image
// ============================================================================

#[test]
fn flatten_image_combines_all_visible_layers() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let a = engine.add_raster_layer(None);
    paint_dot(&mut engine, a, 32.0, 64.0, [1.0, 0.0, 0.0]);
    let b = engine.add_raster_layer(None);
    paint_dot(&mut engine, b, 64.0, 64.0, [0.0, 1.0, 0.0]);
    let c = engine.add_raster_layer(None);
    paint_dot(&mut engine, c, 96.0, 64.0, [0.0, 0.0, 1.0]);

    let result = engine.flatten_image().expect("flatten succeeded");
    assert!(engine.has_layer(result));
    assert!(!engine.has_layer(a));
    assert!(!engine.has_layer(b));
    assert!(!engine.has_layer(c));

    let pixels = engine.test_readback_layer(result);
    assert!(alpha_at(&pixels, w, 32, 64) > 0, "a's dot in flattened");
    assert!(alpha_at(&pixels, w, 64, 64) > 0, "b's dot in flattened");
    assert!(alpha_at(&pixels, w, 96, 64) > 0, "c's dot in flattened");
}

#[test]
fn flatten_undo_restores_original_tree() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let a = engine.add_raster_layer(None);
    paint_dot(&mut engine, a, 16.0, 32.0, [1.0, 0.0, 0.0]);
    let b = engine.add_raster_layer(None);
    paint_dot(&mut engine, b, 48.0, 32.0, [0.0, 1.0, 0.0]);

    let result = engine.flatten_image().expect("flatten succeeded");

    engine.undo();
    assert!(engine.has_layer(a), "a restored");
    assert!(engine.has_layer(b), "b restored");
    assert!(!engine.has_layer(result), "result detached");

    // Source pixels intact.
    let a_px = engine.test_readback_layer(a);
    let b_px = engine.test_readback_layer(b);
    assert!(alpha_at(&a_px, w, 16, 32) > 0, "a pixels intact post-undo");
    assert!(alpha_at(&b_px, w, 48, 32) > 0, "b pixels intact post-undo");
}

// ============================================================================
// Flatten Node (per-layer / per-group)
// ============================================================================

#[test]
fn flatten_node_fails_on_layer_without_mask() {
    let (_, _) = (64u32, 64u32);
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    // No mask attached → flatten_node should error.
    assert!(engine.flatten_node(layer).is_err());
}

#[test]
fn flatten_node_on_layer_with_mask_applies_it() {
    // Sanity: after flatten_node, the layer no longer has a mask modifier.
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [1.0, 0.0, 0.0]);
    engine.add_mask(layer);
    assert!(engine.flatten_node(layer).is_ok());
    assert!(
        engine.host_mask_id(layer).is_none(),
        "mask removed after flatten"
    );
}

#[test]
fn flatten_node_on_group_produces_raster_at_groups_slot() {
    // Group with two children → flatten produces a single raster occupying
    // the group's tree position.
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let group = engine.add_group(None);
    let child_a = engine.add_raster_layer(Some(group));
    paint_dot(&mut engine, child_a, 16.0, 32.0, [1.0, 0.0, 0.0]);
    let child_b = engine.add_raster_layer(Some(group));
    paint_dot(&mut engine, child_b, 48.0, 32.0, [0.0, 1.0, 0.0]);

    let result = engine.flatten_node(group).expect("group flatten succeeded");
    assert_ne!(result, group);
    assert!(engine.has_layer(result));
    assert!(!engine.has_layer(group), "group consumed");

    let pixels = engine.test_readback_layer(result);
    assert!(alpha_at(&pixels, w, 16, 32) > 0, "child_a's dot present");
    assert!(alpha_at(&pixels, w, 48, 32) > 0, "child_b's dot present");
}
