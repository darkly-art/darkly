//! Integration tests for Convert to Smart Object.
//!
//! Run with:
//! `cargo test -p darkly --test convert_to_smart_object --features darkly/testing -- --test-threads=1`
//!
//! **Alpha convention.** A void source is stored 8-bit *premultiplied* and the
//! shader un-premultiplies on the way out (`shaders/voids/textured.wgsl`:
//! `sample.rgb / max(sample.a, 1e-4)`). That round trip is lossy at partial
//! alpha: a straight `(200, α=100)` stores as `round(200·100/255) = 78` and
//! returns as `round(78·255/100) = 199`. So fixtures here are **hard opaque or
//! empty**, where the round trip is exact and `assert_eq!` is sound; anything
//! partial-alpha states an expected value with a tolerance instead.

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

/// A fully opaque `w × h` RGBA block. Straight alpha at 255, so it survives
/// the premultiplied round trip exactly.
fn opaque_block(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    (0..(w * h))
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect()
}

fn rgba_at(pixels: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn is_smart_object(engine: &DarklyEngine, id: LayerId) -> bool {
    use darkly::engine::LayerInfo;
    let target = id.to_ffi() as f64;
    engine.layer_tree().iter().any(|info| {
        matches!(
            info,
            LayerInfo::Void { id, void_type, .. }
                if *id == target && void_type == darkly::gpu::voids::smart_object::TYPE_ID
        )
    })
}

/// Paint an opaque dab so a layer has real content for the alpha-tight
/// extraction to find.
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

/// Start a whole-layer transform session, driving frames until it takes.
///
/// Without a selection the extraction is alpha-tight, and those bounds come
/// from a GPU readback, so the first call reports "not yet" and the real tool
/// retries on the next frame. (With a selection the bounds are known up front
/// and it succeeds immediately, which is why other suites call it once.)
fn begin_transform_settled(engine: &mut DarklyEngine, layer: LayerId) {
    // Called once: each call bumps the setup generation and drops the pending
    // retry, so re-calling in the loop would restart the wait forever. Frames
    // are what advance it: `render` picks the pending setup back up.
    engine.begin_transform(layer);
    for _ in 0..8 {
        // The alpha-content bounds arrive by async readback; flush it, then let
        // a frame pick the pending setup back up.
        engine.test_flush_readbacks();
        engine.render(0.0);
        if engine.has_floating() {
            return;
        }
    }
    panic!(
        "transform session never started (has_floating={}, pending={:?})",
        engine.has_floating(),
        engine.floating_info().is_some(),
    );
}

// ============================================================================
// Floating content → smart object
// ============================================================================

/// The defining property: converting a float must leave the target layer
/// untouched. Committing resamples the source into the target and discards the
/// original; converting keeps the original and makes the transform re-editable,
/// which is worth nothing if the pixels were burned in anyway.
#[test]
fn converting_floating_does_not_write_the_target() {
    let mut engine = test_engine(64, 64);
    let target = engine.add_raster_layer(None);
    engine.render(0.0);
    let target_before = engine.test_readback_layer(target);

    let px = opaque_block(16, 16, [255, 0, 0]);
    engine.paste_image_floating(16, 16, &px, 8, 8, Some(target));

    let id = engine
        .convert_floating_to_smart_object()
        .expect("float converts");
    engine.render(0.0);

    assert!(is_smart_object(&engine, id), "a smart object was created");
    assert_eq!(
        engine.test_readback_layer(target),
        target_before,
        "the target layer must not be written",
    );
    assert!(!engine.has_floating(), "the float is consumed");
}

/// The converted layer must carry the source pixels, at the source's own
/// resolution, or it is a smart object in name only.
#[test]
fn converted_floating_owns_the_source_pixels() {
    let mut engine = test_engine(64, 64);
    let target = engine.add_raster_layer(None);
    let px = opaque_block(16, 16, [255, 0, 0]);
    engine.paste_image_floating(16, 16, &px, 8, 8, Some(target));

    let id = engine
        .convert_floating_to_smart_object()
        .expect("float converts");
    engine.render(0.0);

    let frame = engine
        .test_readback_void_frame(id)
        .expect("the smart object reports a persistent frame");
    assert_eq!(
        frame.len(),
        16 * 16 * 4,
        "the source is held at its own 16x16 resolution, not canvas-sized",
    );
    // Stored premultiplied; opaque red premultiplies to itself.
    assert_eq!(
        rgba_at(&frame, 16, 8, 8),
        [255, 0, 0, 255],
        "the source holds the pasted pixels",
    );
}

/// Undo removes the layer; redo brings it back **with its pixels**.
///
/// Without `sync_void_persistent_frame` the document never records that this
/// void owns a frame, `owns_disposable_texture` reads false, and the tombstone
/// machinery frees the source out from under the redo, leaving a blank layer
/// that no structural assertion would catch.
#[test]
fn convert_floating_is_one_undo_step_and_redo_keeps_the_pixels() {
    let mut engine = test_engine(64, 64);
    let target = engine.add_raster_layer(None);
    let px = opaque_block(16, 16, [0, 255, 0]);
    engine.paste_image_floating(16, 16, &px, 8, 8, Some(target));

    let before_depth = engine.test_transform_commit_observables().0;
    let id = engine
        .convert_floating_to_smart_object()
        .expect("float converts");
    engine.render(0.0);
    let after_depth = engine.test_transform_commit_observables().0;
    assert_eq!(
        after_depth - before_depth,
        1,
        "conversion is exactly one undo step",
    );

    let frame_before = engine.test_readback_void_frame(id).expect("frame");

    engine.undo();
    engine.render(0.0);
    assert!(!engine.has_layer(id), "undo removes the smart object");

    engine.redo();
    engine.render(0.0);
    assert!(engine.has_layer(id), "redo restores it");
    assert_eq!(
        engine
            .test_readback_void_frame(id)
            .expect("frame after redo"),
        frame_before,
        "redo restores the source pixels, not a blank layer",
    );
}

/// A paste with no target auto-creates a placeholder raster. Converting drops
/// it silently: it carries no undo entry until commit, and leaving it behind
/// would litter the panel with empty layers.
#[test]
fn converting_floating_removes_the_paste_placeholder() {
    let mut engine = test_engine(64, 64);
    let px = opaque_block(16, 16, [0, 0, 255]);
    let placeholder = engine.paste_image_floating(16, 16, &px, 8, 8, None);

    let id = engine
        .convert_floating_to_smart_object()
        .expect("float converts");
    engine.render(0.0);

    assert!(
        !engine.has_layer(placeholder),
        "the auto-created target is dropped",
    );
    assert!(engine.has_layer(id));
}

/// Refused with nothing floating; the menu entry is gated on the same
/// predicate, so this is the engine half of that gate.
#[test]
fn converting_with_no_floating_is_refused() {
    let mut engine = test_engine(64, 64);
    let _layer = engine.add_raster_layer(None);
    assert!(!engine.can_convert_floating_to_smart_object());
    assert!(engine.convert_floating_to_smart_object().is_err());
}

// ============================================================================
// Transform session → smart object
//
// This is the case the gizmo actually puts you in most of the time: pick the
// transform tool on a layer and you get a destructive transform *session*, not
// floating paste content. It lifted the layer's pixels and owes the layer a
// hole at commit; converting settles that by consuming the layer outright.
// ============================================================================

/// Transforming a plain layer and converting replaces it in place: the layer
/// is gone, a smart object holds its pixels, and it sits in the same slot.
#[test]
fn converting_a_transformed_layer_replaces_it_with_a_smart_object() {
    let mut engine = test_engine(64, 64);
    let below = engine.add_raster_layer(None);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [1.0, 0.0, 0.0]);

    begin_transform_settled(&mut engine, layer);
    assert!(
        engine.can_convert_floating_to_smart_object(),
        "a whole-layer session with no selection is convertible",
    );

    let id = engine
        .convert_floating_to_smart_object()
        .expect("session converts");
    engine.render(0.0);

    assert!(is_smart_object(&engine, id));
    assert!(!engine.has_layer(layer), "the source layer is consumed");
    assert!(engine.has_layer(below), "siblings are untouched");
    assert!(
        engine.test_readback_void_frame(id).is_some(),
        "the smart object owns its source, so it saves and survives undo",
    );
}

/// One undo step, and undo puts the original layer back with its pixels.
#[test]
fn converting_a_transformed_layer_is_one_undo_step() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [0.0, 1.0, 0.0]);
    let layer_before = engine.test_readback_layer(layer);

    begin_transform_settled(&mut engine, layer);
    let depth_before = engine.test_transform_commit_observables().0;
    let id = engine
        .convert_floating_to_smart_object()
        .expect("session converts");
    engine.render(0.0);
    assert_eq!(
        engine.test_transform_commit_observables().0 - depth_before,
        1,
        "exactly one undo step",
    );

    engine.undo();
    engine.render(0.0);
    assert!(engine.has_layer(layer), "the layer comes back");
    assert!(!engine.has_layer(id), "the smart object goes away");
    assert_eq!(
        engine.test_readback_layer(layer),
        layer_before,
        "the layer's pixels come back intact",
    );
}

/// Start a transform through a left-half selection of a fully red layer.
/// Returns the layer holding the red.
fn begin_half_selected_transform(engine: &mut DarklyEngine) -> LayerId {
    let layer = engine.paste_image(64, 64, &opaque_block(64, 64, [255, 0, 0]), 0, 0, None);
    engine.render(0.0);
    engine.select_rect(
        0.0,
        0.0,
        32.0,
        64.0,
        darkly::document::SelectionMode::Replace,
        false,
        0.0,
    );
    engine.render(0.0);
    assert!(
        engine.begin_transform(layer),
        "selection transform starts at once"
    );
    engine.render(0.0);
    layer
}

/// A selection transform is convertible. Whether a selection is active decides
/// what the conversion *owes the source*, never whether it is allowed: the lift
/// took only the selected pixels, so the rest of the layer stays and the
/// conversion cuts the hole the lift owes it.
#[test]
fn converting_a_selection_transform_keeps_the_unlifted_remainder() {
    let mut engine = test_engine(64, 64);
    let layer = begin_half_selected_transform(&mut engine);

    assert!(
        engine.can_convert_floating_to_smart_object(),
        "a selection transform is convertible",
    );
    let id = engine
        .convert_floating_to_smart_object()
        .expect("selection session converts");
    engine.render(0.0);

    assert!(is_smart_object(&engine, id));
    assert!(
        engine.has_layer(layer),
        "the source layer survives, only part of it was lifted",
    );

    // Erase zeroes alpha and leaves the colour channels alone (layers store
    // straight alpha), so the hole reads as the original red at zero coverage:
    // the same residue an ordinary commit's clear leaves.
    let px = engine.test_readback_layer(layer);
    assert_eq!(
        rgba_at(&px, 64, 8, 32),
        [255, 0, 0, 0],
        "the lifted half is left as a hole",
    );
    assert_eq!(
        rgba_at(&px, 64, 48, 32),
        [255, 0, 0, 255],
        "the unselected half is untouched",
    );
    assert!(
        engine.test_readback_void_frame(id).is_some(),
        "the smart object owns the lifted pixels",
    );
}

/// The smart object must render at its source's natural size the moment it is
/// created, not on the next transform.
///
/// Installing a source by GPU blit flips the void off its canvas-covering
/// placeholder, so the sampling uniform written when the layer was created
/// describes the wrong extent until it is rewritten. Nothing between creation
/// and the first drag rewrites it, so the image ships stretched to the canvas,
/// and the gizmo reads the *document* extent, so it looks correct while the
/// pixels are wrong.
#[test]
fn a_converted_smart_object_renders_at_its_source_size() {
    let mut engine = test_engine(64, 64);
    let layer = engine.paste_image(16, 16, &opaque_block(16, 16, [0, 255, 0]), 8, 8, None);
    engine.render(0.0);

    engine.select_rect(
        8.0,
        8.0,
        16.0,
        16.0,
        darkly::document::SelectionMode::Replace,
        false,
        0.0,
    );
    engine.render(0.0);
    assert!(engine.begin_transform(layer), "transform starts");
    engine.render(0.0);

    let id = engine
        .convert_floating_to_smart_object()
        .expect("session converts");
    engine.render(0.0);
    assert!(is_smart_object(&engine, id));

    let canvas = engine.test_readback_canvas();
    assert_eq!(
        rgba_at(&canvas, 64, 16, 16),
        [0, 255, 0, 255],
        "the source lands where it was lifted from",
    );
    assert_eq!(
        rgba_at(&canvas, 64, 48, 48)[3],
        0,
        "and nowhere else; a stretched source would cover this",
    );
}

/// The hole and the new layer are one edit: undo puts the pixels back and takes
/// the smart object away together, or the user is left with a half-erased layer.
#[test]
fn converting_a_selection_transform_is_one_undo_step() {
    let mut engine = test_engine(64, 64);
    let layer = begin_half_selected_transform(&mut engine);
    let before = engine.test_readback_layer(layer);
    let depth_before = engine.test_transform_commit_observables().0;

    let id = engine
        .convert_floating_to_smart_object()
        .expect("selection session converts");
    engine.render(0.0);
    assert_eq!(
        engine.test_transform_commit_observables().0 - depth_before,
        1,
        "exactly one undo step",
    );

    engine.undo();
    engine.render(0.0);
    assert!(!engine.has_layer(id), "undo removes the smart object");
    assert_eq!(
        engine.test_readback_layer(layer),
        before,
        "undo restores the lifted pixels in the same step",
    );
}

// ============================================================================
// Layer → smart object (layer panel)
// ============================================================================

/// The layer-panel conversion is not a transform: the layer keeps its exact
/// appearance, its pixels move into the smart object's source untouched, and
/// the source layer is consumed.
#[test]
fn converting_a_layer_replaces_it_with_a_smart_object() {
    let mut engine = test_engine(64, 64);
    let below = engine.add_raster_layer(None);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [0.0, 1.0, 0.0]);
    engine.render(0.0);

    assert!(
        engine.can_convert_layer_to_smart_object(layer),
        "a painted raster layer is convertible",
    );
    let id = engine
        .convert_layer_to_smart_object(layer)
        .expect("layer converts");
    engine.render(0.0);

    assert!(is_smart_object(&engine, id));
    assert!(!engine.has_layer(layer), "the source layer is consumed");
    assert!(engine.has_layer(below), "siblings are untouched");
}

/// Conversion must be visually inert: the whole promise is "same picture, now
/// scalable". The composite before and after has to match pixel for pixel, or
/// the transform that re-anchors the source is wrong.
#[test]
fn converting_a_layer_does_not_move_or_alter_it() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 20.0, 44.0, [1.0, 0.0, 0.0]);
    engine.render(0.0);
    let before = engine.test_readback_canvas();

    let id = engine
        .convert_layer_to_smart_object(layer)
        .expect("layer converts");
    engine.render(0.0);
    let after = engine.test_readback_canvas();

    assert!(is_smart_object(&engine, id));
    assert_eq!(
        before, after,
        "the canvas must look identical after conversion",
    );
}

/// Undo restores the source layer and its pixels in one step, the same
/// contract the transform-session conversion honours.
#[test]
fn converting_a_layer_is_one_undo_step() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [0.0, 0.0, 1.0]);
    engine.render(0.0);
    let layer_before = engine.test_readback_layer(layer);

    let id = engine
        .convert_layer_to_smart_object(layer)
        .expect("layer converts");
    engine.render(0.0);

    engine.undo();
    engine.render(0.0);
    assert!(engine.has_layer(layer), "the layer comes back");
    assert!(!engine.has_layer(id), "the smart object goes away");
    assert_eq!(
        engine.test_readback_layer(layer),
        layer_before,
        "the layer's pixels come back intact",
    );
}

/// A layer that is already a smart object has nothing to gain and a pristine
/// source to lose, so the row must not offer the conversion again.
#[test]
fn converting_an_existing_smart_object_is_refused() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [1.0, 1.0, 0.0]);
    engine.render(0.0);
    let id = engine
        .convert_layer_to_smart_object(layer)
        .expect("layer converts");
    engine.render(0.0);

    assert!(
        !engine.can_convert_layer_to_smart_object(id),
        "a smart object must not be offered conversion",
    );
    assert!(engine.convert_layer_to_smart_object(id).is_err());
    assert!(
        engine.has_layer(id),
        "the smart object survives the refusal"
    );
}

/// The conversion moves the layer's own texture, not its composite, so a mask
/// would vanish without a trace. Refused rather than silently discarded.
#[test]
fn converting_a_masked_layer_is_refused() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [0.0, 1.0, 1.0]);
    engine.add_mask(layer);
    engine.render(0.0);

    assert!(
        !engine.can_convert_layer_to_smart_object(layer),
        "a masked layer must not be offered conversion",
    );
    assert!(engine.convert_layer_to_smart_object(layer).is_err());
    assert!(engine.has_layer(layer), "the layer survives the refusal");
}

/// A group has no texture of its own; baking one would cap it at canvas
/// resolution, which is the opposite of what a smart object is for.
#[test]
fn converting_a_group_is_refused() {
    let mut engine = test_engine(64, 64);
    let group = engine.add_group(None);
    engine.render(0.0);
    let tree_before = engine.layer_tree().len();

    assert!(!engine.can_convert_layer_to_smart_object(group));
    assert!(engine.convert_layer_to_smart_object(group).is_err());
    assert_eq!(
        engine.layer_tree().len(),
        tree_before,
        "the tree is untouched by the refusal",
    );
}

/// A locked layer is not editable, and every other structural menu entry
/// respects that; this one must too.
#[test]
fn converting_a_locked_layer_is_refused() {
    let mut engine = test_engine(64, 64);
    let layer = engine.add_raster_layer(None);
    paint_dot(&mut engine, layer, 32.0, 32.0, [1.0, 0.0, 1.0]);
    engine.render(0.0);
    engine.set_node_locked(layer, true);

    assert!(!engine.can_convert_layer_to_smart_object(layer));
    assert!(engine.convert_layer_to_smart_object(layer).is_err());
    assert!(engine.has_layer(layer), "the layer survives the refusal");
}
