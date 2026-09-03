//! Integration tests for Duplicate / Merge Down / Flatten Image.
//!
//! Run with: `cargo test -p darkly --test layer_bake -- --test-threads=1`

use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::engine::LayerInfo;
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
    engine.begin_stroke(layer_id).unwrap();
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

/// Paint a black dab onto a host's mask filter: R8 value drops toward 0
/// where the brush lands, leaving the rest of the mask at its prior value
/// (255 for a freshly-added, all-reveal mask).
fn paint_mask_dot(engine: &mut DarklyEngine, host_id: LayerId, x: f32, y: f32) {
    let mask_id = engine
        .host_mask_id(host_id)
        .expect("paint_mask_dot requires the host to have a mask filter");
    engine.begin_stroke(mask_id).unwrap();
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 0.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

/// True iff `group_id` appears as a direct child of the document root in
/// the engine's published layer tree. Lets group-presence be checked from
/// integration tests without leaking `pub(crate)` doc internals.
fn group_at_root(engine: &DarklyEngine, group_id: LayerId) -> bool {
    use darkly::engine::LayerInfo;
    let target = group_id.to_ffi() as f64;
    engine
        .layer_tree()
        .iter()
        .any(|info| matches!(info, LayerInfo::Group { id, .. } if *id == target))
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

    // Source pixels must be intact: tombstoning kept textures alive.
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
// Thumbnail auto-queue regression
// ============================================================================
//
// Protects the "every write-site marks its node thumbnail-dirty" invariant
// (see `Compositor::mark_node_pixels_dirty` docs). Without that, a fresh
// duplicate appears in the panel as a thumbnail-less row until the artist
// makes their first edit: the original bug this refactor was written to
// kill, recurring "the fourth or fifth time" in the codebase's history.

#[test]
fn duplicate_marks_new_layer_thumbnail_dirty() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_a = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer_a, 32.0, 32.0, [1.0, 0.0, 0.0]);
    engine.render(0.0);
    engine.test_flush_readbacks(); // Settle any startup readbacks.

    let layer_b = engine.duplicate_node(layer_a).expect("duplicate");

    // `render` drains the dirty set into a readback request; the flush
    // forces the async readback to land in `thumbnail_cache` deterministically.
    engine.render(0.016);
    engine.test_flush_readbacks();

    let bytes = engine
        .test_thumbnail_cache_peek(layer_b)
        .expect("duplicate must have queued a thumbnail readback automatically");
    assert!(
        bytes.iter().any(|&v| v != 0),
        "duplicated layer's thumbnail must contain non-zero pixels without a manual edit"
    );
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
    // Sanity: after flatten_node, the layer no longer has a mask filter.
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

#[test]
fn flatten_group_with_masks_undo_restores_tree_and_pixels() {
    // Tree under test:
    //   Group (with mask, dab at 8,8)
    //     ├─ child_a (with mask, dab at 16,16; red dot at 16,32)
    //     └─ child_b (green dot at 48,32)
    //
    // Flattening the group must consume every child and every mask. Undo
    // must put all of it back: tree shape, both masks, and every pixel
    // byte-for-byte.
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);

    let group = engine.add_group(None);
    let child_a = engine.add_raster_layer(Some(group));
    paint_dot(&mut engine, child_a, 16.0, 32.0, [1.0, 0.0, 0.0]);
    engine.add_mask(child_a);
    paint_mask_dot(&mut engine, child_a, 16.0, 16.0);

    let child_b = engine.add_raster_layer(Some(group));
    paint_dot(&mut engine, child_b, 48.0, 32.0, [0.0, 1.0, 0.0]);

    engine.add_mask(group);
    paint_mask_dot(&mut engine, group, 8.0, 8.0);

    // Snapshot every pixel buffer we expect to survive the round-trip.
    let child_a_pixels_before = engine.test_readback_layer(child_a);
    let child_b_pixels_before = engine.test_readback_layer(child_b);
    let child_a_mask_before = engine.test_readback_mask(child_a);
    let group_mask_before = engine.test_readback_mask(group);

    // --- Forward ---
    let result = engine.flatten_node(group).expect("group flatten succeeded");
    assert!(engine.has_layer(result), "result raster attached");
    assert!(!group_at_root(&engine, group), "group consumed by flatten");
    // Result composite must reflect both children, proof the bake actually
    // walked the subtree, rather than emitting an empty placeholder.
    let result_pixels = engine.test_readback_layer(result);
    assert!(
        alpha_at(&result_pixels, w, 16, 32) > 0,
        "child_a's dot baked into result"
    );
    assert!(
        alpha_at(&result_pixels, w, 48, 32) > 0,
        "child_b's dot baked into result"
    );

    // --- Undo ---
    engine.undo();

    // Tree shape: group is back at root, both children attached under it,
    // result raster gone, both masks reattached.
    assert!(group_at_root(&engine, group), "group reattached at root");
    assert!(engine.has_layer(child_a), "child_a reattached");
    assert!(engine.has_layer(child_b), "child_b reattached");
    assert!(!engine.has_layer(result), "result detached on undo");
    assert!(
        engine.host_mask_id(group).is_some(),
        "group's mask restored"
    );
    assert!(
        engine.host_mask_id(child_a).is_some(),
        "child_a's mask restored"
    );

    // Pixel content: every snapshot matches byte-for-byte.
    assert_eq!(
        engine.test_readback_layer(child_a),
        child_a_pixels_before,
        "child_a layer pixels survive flatten+undo byte-for-byte"
    );
    assert_eq!(
        engine.test_readback_layer(child_b),
        child_b_pixels_before,
        "child_b layer pixels survive flatten+undo byte-for-byte"
    );
    assert_eq!(
        engine.test_readback_mask(child_a),
        child_a_mask_before,
        "child_a mask pixels survive flatten+undo byte-for-byte"
    );
    assert_eq!(
        engine.test_readback_mask(group),
        group_mask_before,
        "group mask pixels survive flatten+undo byte-for-byte"
    );
}

// ============================================================================
// merge_layers: multi-source bake, same-parent and cross-parent
// ============================================================================

/// Merging a same-parent selection lands the result at the panel-topmost
/// selected sibling's slot and inherits that sibling's name / blend mode /
/// visibility.
///
/// Opacity is the exception, and the assertion below is deliberate: the
/// topmost's opacity is baked into the result's pixels, so the result carries
/// 100 % and composites to the same image. See
/// `merge_layers_does_not_double_the_topmosts_opacity`.
#[test]
fn merge_layers_same_parent_inherits_topmost_props() {
    use darkly::engine::types::LayerInfo;
    let mut engine = test_engine(32, 32);
    let _keep = engine.add_raster_layer(None);
    let lower = engine.add_raster_layer(None);
    let upper = engine.add_raster_layer(None);
    // Rename the topmost so we can confirm the result inherits its name.
    engine.set_layer_name(upper, "topmost");
    engine.set_opacity(upper, 0.5);

    paint_dot(&mut engine, lower, 8.0, 8.0, [1.0, 0.0, 0.0]);
    paint_dot(&mut engine, upper, 16.0, 16.0, [0.0, 1.0, 0.0]);

    // What the user sees before the merge. Merging is a restructuring, not an
    // edit, so this must survive it unchanged: the assertion that actually
    // catches a doubled opacity, whatever the tree ends up looking like.
    engine.render(0.0);
    let before = engine.test_readback_canvas();

    let result = engine.merge_layers(vec![lower, upper]).expect("merge ok");

    // Source layers should be detached from the tree.
    assert!(!engine.has_layer(lower));
    assert!(!engine.has_layer(upper));
    assert!(engine.has_layer(result));

    // Result inherits the topmost's name; its opacity is in the pixels.
    let info = engine
        .layer_tree()
        .into_iter()
        .find(|n| match n {
            LayerInfo::Raster { id, .. } => *id == result.to_ffi() as f64,
            _ => false,
        })
        .expect("result in tree");
    let (name, opacity) = match info {
        LayerInfo::Raster { name, opacity, .. } => (name, opacity),
        _ => panic!(),
    };
    assert_eq!(name, "topmost");
    assert!(
        (opacity - 1.0).abs() < 1e-3,
        "the topmost's opacity is baked into the pixels, so the result must \
         carry 100% or it would be applied twice: {opacity}",
    );

    engine.render(0.0);
    assert_eq!(
        engine.test_readback_canvas(),
        before,
        "merging must not change the composite",
    );
}

/// A cross-parent selection (one layer at root, one inside a group)
/// merges successfully: result lands at the topmost selected layer's
/// slot, and all sources are detached.
#[test]
fn merge_layers_cross_parent_lands_at_topmost() {
    use darkly::document::MoveTarget;
    use darkly::engine::types::LayerInfo;
    let mut engine = test_engine(32, 32);
    let _keep = engine.add_raster_layer(None);
    // Build [keep, in_group_via_group, group(inner), root_top].
    let group = engine.add_group(None);
    let inner = engine.add_raster_layer(None);
    engine.move_layer(inner, MoveTarget::IntoGroupTop(group));
    let root_top = engine.add_raster_layer(None);
    engine.set_layer_name(root_top, "expected-topmost");

    paint_dot(&mut engine, inner, 8.0, 8.0, [1.0, 0.0, 0.0]);
    paint_dot(&mut engine, root_top, 16.0, 16.0, [0.0, 0.0, 1.0]);

    let result = engine
        .merge_layers(vec![inner, root_top])
        .expect("cross-parent merge ok");

    assert!(!engine.has_layer(inner), "inner source detached");
    assert!(!engine.has_layer(root_top), "root_top source detached");
    assert!(engine.has_layer(result));

    // Result inherits root_top's name (root_top is the panel-topmost of
    // the selection: it's the LAST entry of all_node_ids_in_order that
    // matches an id in the selection).
    let info = engine
        .layer_tree()
        .into_iter()
        .find(|n| match n {
            LayerInfo::Raster { id, .. } => *id == result.to_ffi() as f64,
            _ => false,
        })
        .expect("result in tree");
    let name = match info {
        LayerInfo::Raster { name, .. } => name,
        _ => panic!(),
    };
    assert_eq!(name, "expected-topmost");
}

/// A single undo step restores all merged sources, even when they
/// straddle different parent groups.
#[test]
fn merge_layers_undo_restores_all_sources() {
    use darkly::document::MoveTarget;
    let mut engine = test_engine(32, 32);
    let _keep = engine.add_raster_layer(None);
    let group = engine.add_group(None);
    let inner = engine.add_raster_layer(None);
    engine.move_layer(inner, MoveTarget::IntoGroupTop(group));
    let root_top = engine.add_raster_layer(None);

    paint_dot(&mut engine, inner, 8.0, 8.0, [1.0, 0.0, 0.0]);
    paint_dot(&mut engine, root_top, 16.0, 16.0, [0.0, 0.0, 1.0]);

    let inner_before = engine.test_readback_layer(inner);
    let root_top_before = engine.test_readback_layer(root_top);

    let result = engine
        .merge_layers(vec![inner, root_top])
        .expect("merge ok");
    assert!(engine.has_layer(result));
    assert!(!engine.has_layer(inner));
    assert!(!engine.has_layer(root_top));

    engine.undo();
    engine.render(0.0);

    assert!(engine.has_layer(inner), "inner source restored");
    assert!(engine.has_layer(root_top), "root_top source restored");
    assert!(!engine.has_layer(result), "result removed by undo");
    assert_eq!(
        engine.test_readback_layer(inner),
        inner_before,
        "inner pixels restored byte-for-byte"
    );
    assert_eq!(
        engine.test_readback_layer(root_top),
        root_top_before,
        "root_top pixels restored byte-for-byte"
    );
}

/// `merge_layers` aborts if any source is locked, since a partial bake
/// would destroy the artist's data.
#[test]
fn merge_layers_rejects_locked() {
    let mut engine = test_engine(32, 32);
    let _keep = engine.add_raster_layer(None);
    let l1 = engine.add_raster_layer(None);
    let l2 = engine.add_raster_layer(None);
    engine.set_node_locked(l1, true);

    let result = engine.merge_layers(vec![l1, l2]);
    assert!(result.is_err(), "locked source must reject the merge");
    assert!(engine.has_layer(l1));
    assert!(engine.has_layer(l2));
}

/// `merge_layers` needs at least two distinct sources.
#[test]
fn merge_layers_needs_two_sources() {
    let mut engine = test_engine(32, 32);
    let _keep = engine.add_raster_layer(None);
    let l1 = engine.add_raster_layer(None);

    let r1 = engine.merge_layers(vec![l1]);
    assert!(r1.is_err(), "single-id merge must error");

    let r2 = engine.merge_layers(vec![l1, l1]);
    assert!(r2.is_err(), "duplicate-id merge must error (dedupes to 1)");
}

/// Regression: merging must not apply the target's opacity twice.
///
/// The bake composites each source through its own blend uniforms, so a 50 %
/// layer already contributes at 50 % in the accumulated result. Copying that
/// same opacity onto the result layer applies it a second time at composite,
/// so a merged 50 % layer used to read 25 %. The result inherits the target's
/// blend *mode* (which is a no-op against the cleared accumulator and so is
/// not baked in), but its opacity is already in the pixels.
#[test]
fn merge_down_does_not_double_the_targets_opacity() {
    let mut engine = test_engine(32, 32);
    let target = engine.add_raster_layer(None);
    paint_dot(&mut engine, target, 16.0, 16.0, [1.0, 0.0, 0.0]);
    engine.set_opacity(target, 0.5);

    // An empty layer above: merge_down composites `[target, source]`, so the
    // result is the target's pixels at 50 % and nothing else.
    let source = engine.add_raster_layer(None);
    engine.merge_down(source).expect("merge_down");
    engine.render(0.0);

    let px = engine.test_readback_canvas();
    assert_eq!(
        alpha_at(&px, 32, 16, 16),
        128,
        "a merged 50 % layer must composite at 50 %, not 25 %",
    );
}

/// Sibling of the above for the multi-select path, which carries the same
/// inheritance. Two opaque layers merged with the topmost at 50 %: the bake
/// puts the topmost over the bottom at 50 %, which is already fully opaque
/// where they overlap, so the result must be opaque.
#[test]
fn merge_layers_does_not_double_the_topmosts_opacity() {
    let mut engine = test_engine(32, 32);
    let bottom = engine.add_raster_layer(None);
    paint_dot(&mut engine, bottom, 16.0, 16.0, [0.0, 0.0, 1.0]);
    let top = engine.add_raster_layer(None);
    paint_dot(&mut engine, top, 16.0, 16.0, [1.0, 0.0, 0.0]);
    engine.set_opacity(top, 0.5);

    engine
        .merge_layers(vec![bottom, top])
        .expect("merge_layers");
    engine.render(0.0);

    let px = engine.test_readback_canvas();
    assert_eq!(
        alpha_at(&px, 32, 16, 16),
        255,
        "opaque-under-translucent merges to opaque; the topmost's opacity is \
         already in the baked pixels",
    );
}

// ============================================================================
// Guards for the shared bake tail
//
// Every bake op ends in the same sequence: tombstone the sources, detach them,
// land the result in a specific slot, push one `BakeLayersAction`. These four
// tests pin the parts of that sequence nothing else asserted: the result's
// *position*, redo through the action, hidden-source tombstoning, and that the
// undo entry is a single step. Without them a wrong slot or a dropped source
// passes the rest of this file unnoticed.
// ============================================================================

/// Flatten Image puts its result at the bottom of the root stack: the
/// Photoshop "Background" convention. `layer_tree()` is top-to-bottom, so the
/// result is the LAST entry.
#[test]
fn flatten_image_result_lands_at_the_bottom_of_root() {
    let mut engine = test_engine(32, 32);
    let a = engine.add_raster_layer(None);
    paint_dot(&mut engine, a, 8.0, 8.0, [1.0, 0.0, 0.0]);
    let b = engine.add_raster_layer(None);
    paint_dot(&mut engine, b, 16.0, 16.0, [0.0, 1.0, 0.0]);

    let result = engine.flatten_image().expect("flatten succeeded");

    let tree = engine.layer_tree();
    assert_eq!(tree.len(), 1, "flatten leaves exactly one root node");
    let last = tree.last().expect("non-empty");
    let id = match last {
        LayerInfo::Raster { id, .. } => *id,
        _ => panic!("expected a raster at the bottom of root"),
    };
    assert_eq!(id, result.to_ffi() as f64, "result is the bottom-most node");
}

/// Merge Down lands its result in the *target's* slot, not the source's, and
/// leaves every unrelated sibling where it was.
#[test]
fn merge_down_result_takes_the_targets_slot() {
    let mut engine = test_engine(32, 32);
    // Bottom-to-top: keep_low, target, source, keep_high.
    let keep_low = engine.add_raster_layer(None);
    let target = engine.add_raster_layer(None);
    let source = engine.add_raster_layer(None);
    let keep_high = engine.add_raster_layer(None);
    paint_dot(&mut engine, target, 8.0, 8.0, [1.0, 0.0, 0.0]);
    paint_dot(&mut engine, source, 16.0, 16.0, [0.0, 1.0, 0.0]);

    let result = engine.merge_down(source).expect("merge_down");

    // Top-to-bottom: keep_high, result, keep_low.
    let ids: Vec<f64> = engine
        .layer_tree()
        .iter()
        .map(|n| match n {
            LayerInfo::Raster { id, .. } => *id,
            _ => panic!("expected only rasters at root"),
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            keep_high.to_ffi() as f64,
            result.to_ffi() as f64,
            keep_low.to_ffi() as f64,
        ],
        "result occupies the target's slot, siblings undisturbed",
    );
    assert!(!engine.has_layer(target));
}

/// Undo then redo through a `BakeLayersAction` must return the document to the
/// post-bake state, pixels included; nothing else in this file drives redo
/// through a bake.
#[test]
fn merge_down_redo_restores_the_merged_result() {
    let mut engine = test_engine(32, 32);
    let target = engine.add_raster_layer(None);
    let source = engine.add_raster_layer(None);
    paint_dot(&mut engine, target, 8.0, 8.0, [1.0, 0.0, 0.0]);
    paint_dot(&mut engine, source, 16.0, 16.0, [0.0, 1.0, 0.0]);

    let result = engine.merge_down(source).expect("merge_down");
    engine.render(0.0);
    let merged = engine.test_readback_canvas();

    engine.undo();
    assert!(engine.has_layer(target), "sources back");
    assert!(engine.has_layer(source), "sources back");

    engine.redo();
    engine.render(0.0);
    assert!(engine.has_layer(result), "result back after redo");
    assert!(!engine.has_layer(target), "sources detached again");
    assert_eq!(
        engine.test_readback_canvas(),
        merged,
        "redo restores the merged pixels, not a blank layer",
    );
}

/// Flatten discards hidden layers but must still tombstone them, so undo can
/// bring them back with their pixels. Nothing else here flattens with a hidden
/// source, which is the path where a dropped tombstone would show up.
#[test]
fn flatten_undo_restores_a_hidden_source_with_its_pixels() {
    let mut engine = test_engine(32, 32);
    let visible = engine.add_raster_layer(None);
    paint_dot(&mut engine, visible, 8.0, 8.0, [1.0, 0.0, 0.0]);
    let hidden = engine.add_raster_layer(None);
    paint_dot(&mut engine, hidden, 16.0, 16.0, [0.0, 1.0, 0.0]);
    let hidden_px = engine.test_readback_layer(hidden);
    engine.set_layer_visible(hidden, false);

    let result = engine.flatten_image().expect("flatten succeeded");
    assert!(
        !engine.has_layer(hidden),
        "hidden source detached by flatten"
    );

    engine.undo();
    assert!(engine.has_layer(hidden), "hidden source restored");
    assert!(!engine.has_layer(result));
    assert_eq!(
        engine.test_readback_layer(hidden),
        hidden_px,
        "a hidden source's pixels survive the round trip",
    );
}
