//! Regression tests for the copy/paste ↔ mask interaction.
//!
//! Model: plain paste always makes its own layer; pasting INTO the active
//! target (a layer or a mask) is the "paste in place" verb, which writes RGBA
//! layers and R8 masks alike. Target kind never changes the routing — with
//! transform-after-paste on, every target floats so the clip can be positioned
//! before it overwrites anything, and the transform tool is what commits it.
//! An RGBA clip converts to mask values on the way in (luminance, transparency
//! resolving to white); see `gpu::transform::rgba_to_mask_values`.
//! The defects covered:
//!   1. Paste-in-place must write into the active mask (not silently no-op,
//!      and not create a new layer), and be undoable.
//!   2. Copying a *region* (selection active) of a masked layer must produce a
//!      flat paste with no mask.
//!   3. Pasting must never seed the restored mask from the receiving document's
//!      active selection.
//!
//! Run with: `cargo test -p darkly --test paste_mask -- --test-threads=1`

use darkly::document::SelectionMode;
use darkly::engine::types::{LayerInfo, StrokeOp};
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Paint a dab of the given straight-alpha colour and render one frame.
fn paint_dot(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32, rgb: (f32, f32, f32)) {
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
        cr: rgb.0,
        cg: rgb.1,
        cb: rgb.2,
        ca: 1.0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

/// Drive any in-flight async readbacks to completion.
fn settle(engine: &mut DarklyEngine) {
    for _ in 0..8 {
        engine.test_flush_readbacks();
        engine.render(0.0);
    }
}

/// Copy `src`'s pixels into the internal clipboard (async readback drained),
/// so the paste verbs have something to paste.
fn copy_into_clipboard(engine: &mut DarklyEngine, src: LayerId) {
    engine.copy(src);
    settle(engine);
}

/// Block until the rich-copy readback completes and return its JSON.
fn drain_rich_copy(engine: &mut DarklyEngine) -> String {
    settle(engine);
    engine
        .poll_copy_rich_result()
        .expect("rich copy never produced a result")
}

/// `(name, opacity, blend_mode, modifier_count)` for a top-level raster layer.
fn raster_props(engine: &DarklyEngine, id: LayerId) -> (String, f32, String, usize) {
    let id_f = id.to_ffi() as f64;
    for info in engine.layer_tree() {
        if let LayerInfo::Raster {
            id: lid,
            name,
            opacity,
            blend_mode,
            modifiers,
            ..
        } = info
        {
            if (lid - id_f).abs() < 0.5 {
                return (name, opacity, blend_mode.to_string(), modifiers.len());
            }
        }
    }
    panic!("layer {id_f} not found in tree");
}

/// Bug 1 (floating verb): floating the clipboard onto a MASK and committing it
/// writes into the mask — no new layer, and undoable. This is the path the UI
/// takes when "activate transform after paste" is on; the commit below stands
/// in for the gizmo gesture that ends the session.
#[test]
fn paste_in_place_floating_writes_into_active_mask() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);

    // A source layer to copy from, plus the masked host we paste into.
    let src = e.add_raster_layer(None);
    paint_dot(&mut e, src, 32.0, 32.0, (0.25, 0.78, 0.13));
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.test_mask_id(host).expect("host has a mask filter");
    settle(&mut e);

    copy_into_clipboard(&mut e, src);

    // Baseline: the fresh mask is uniform.
    let base = e.test_readback_mask(host);
    let v0 = base[0];
    assert!(base.iter().all(|&v| v == v0), "fresh mask must be uniform");

    let layers_before = e.layer_tree().len();
    assert!(
        e.paste_in_place_floating(mask),
        "paste-in-place must float the clipboard onto the mask (not no-op)"
    );
    e.commit_floating();
    settle(&mut e);

    assert_eq!(
        e.layer_tree().len(),
        layers_before,
        "pasting into a mask must not add a layer"
    );
    let after = e.test_readback_mask(host);
    assert!(
        after.iter().any(|&v| v != v0),
        "paste-in-place must write pixels into the mask"
    );

    // Undoable: the mask returns to its pre-paste (uniform) state.
    e.undo();
    settle(&mut e);
    let undone = e.test_readback_mask(host);
    assert!(
        undone.iter().all(|&v| v == v0),
        "undo must restore the mask to its pre-paste pixels"
    );
}

/// Bug 1 (committed verb): with transform-after-paste off, paste-in-place
/// commits the clipboard straight into the active mask.
#[test]
fn paste_in_place_committed_writes_into_active_mask() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);

    let src = e.add_raster_layer(None);
    paint_dot(&mut e, src, 32.0, 32.0, (0.25, 0.78, 0.13));
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.test_mask_id(host).expect("host has a mask filter");
    settle(&mut e);

    copy_into_clipboard(&mut e, src);

    let base = e.test_readback_mask(host);
    let v0 = base[0];
    let layers_before = e.layer_tree().len();

    let target = e
        .paste_in_place(Some(mask))
        .expect("committed paste-in-place must succeed");
    assert_eq!(target, mask, "committed paste-in-place targets the mask");
    settle(&mut e);

    assert_eq!(
        e.layer_tree().len(),
        layers_before,
        "committed paste into a mask must not add a layer"
    );
    let after = e.test_readback_mask(host);
    assert!(
        after.iter().any(|&v| v != v0),
        "committed paste-in-place must write pixels into the mask"
    );
}

/// REPRO: paste a pure-GREEN clip into a mask. A mask is a grayscale value, so
/// the clip must convert by luminance (green ≈ 0.7152 → ~182). Reading the
/// source's red channel instead yields 0 — the mask goes black and the host
/// vanishes, "it pastes full black regardless of what is in the clipboard".
#[test]
fn repro_rgba_paste_into_mask_converts_by_luminance_not_red() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);

    let src = e.add_raster_layer(None);
    paint_dot(&mut e, src, 32.0, 32.0, (0.0, 1.0, 0.0));
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.test_mask_id(host).expect("host has a mask filter");
    settle(&mut e);

    copy_into_clipboard(&mut e, src);

    assert!(
        e.paste_in_place_floating(mask),
        "paste must float onto the mask"
    );
    e.commit_floating();
    settle(&mut e);

    let after = e.test_readback_mask(host);
    let ext = e.test_node_extent(mask);
    let stride = ext.width as usize;
    let at = |x: i32, y: i32| after[(y - ext.y0()) as usize * stride + (x - ext.x0()) as usize];

    // Under the dab, pure green converts by luminance: 0.7152 → ~182. Reading
    // the source's red channel instead gives 0 and blacks the mask out.
    let expected = (0.7152f32 * 255.0).round() as u8;
    let under_dab = at(32, 32);
    assert!(
        under_dab.abs_diff(expected) <= 6,
        "pure green must convert to its luminance (~{expected}); the mask reads \
         {under_dab} under the dab"
    );
    // Away from the dab the clip is transparent, and transparent writes nothing.
    assert_eq!(
        at(2, 2),
        255,
        "a transparent part of the clip must leave the mask as it was"
    );
}

/// The floating session's box is the pasted *object*, not the region the copy
/// swept. A select-all copy of a layer holding one small dab used to hand the
/// transform gizmo a canvas-sized box around it.
#[test]
fn floating_paste_box_is_the_content_not_the_copied_region() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);

    let src = e.add_raster_layer(None);
    paint_dot(&mut e, src, 32.0, 32.0, (0.0, 1.0, 0.0));
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.test_mask_id(host).expect("host has a mask filter");
    settle(&mut e);

    // Select-all, so the copied region is the entire canvas.
    e.select_all();
    settle(&mut e);
    e.copy_layer_rich(src);
    settle(&mut e);

    assert!(
        e.paste_in_place_floating(mask),
        "paste must float onto the mask"
    );
    let (x, y, fw, fh, _) = e.floating_info().expect("a floating session is active");

    assert!(
        fw < w as f32 && fh < h as f32,
        "the floating box must be the dab's bounds, not the {w}×{h} region that \
         was copied; got {fw}×{fh}"
    );
    // The dab is centred at (32, 32), so its box must contain that point.
    assert!(
        x <= 32.0 && y <= 32.0 && x + fw > 32.0 && y + fh > 32.0,
        "the floating box ({x}, {y}, {fw}, {fh}) must contain the dab at (32, 32)"
    );
}

/// A mask paste is a real floating session: it can be repositioned before it
/// lands. Pasting into a mask used to commit the moment the key was pressed,
/// which both skipped the transform gizmo and clobbered the mask outright.
/// Translating the floating must move where the clip comes down.
#[test]
fn floating_paste_into_mask_lands_where_it_was_moved_to() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);

    // A dab up at (16, 16); the clipboard clip is trimmed to it.
    let src = e.add_raster_layer(None);
    paint_dot(&mut e, src, 16.0, 16.0, (0.0, 1.0, 0.0));
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.test_mask_id(host).expect("host has a mask filter");
    settle(&mut e);

    copy_into_clipboard(&mut e, src);

    assert!(
        e.paste_in_place_floating(mask),
        "paste must open a floating session on the mask"
    );
    // The transform tool gates its gizmo on this; "none" would leave a mask
    // paste floating with no handles to position or commit it by.
    assert_eq!(
        e.layer_transform_capability(mask),
        "destructive",
        "a mask must be transformable, or the paste gets no gizmo"
    );
    // Drag it down-right by (32, 32) before committing, as the gizmo would.
    e.update_floating_matrix(darkly::transform::Transform::from_affine(
        darkly::gpu::transform::affine_translate(32.0, 32.0),
    ));
    e.commit_floating();
    settle(&mut e);

    // Readback rows are texture-local, so canvas coords go through the mask
    // texture's own extent rather than the canvas dimensions.
    let after = e.test_readback_mask(host);
    let ext = e.test_node_extent(mask);
    let stride = ext.width as usize;
    let at = |x: i32, y: i32| after[(y - ext.y0()) as usize * stride + (x - ext.x0()) as usize];

    // The clip's transparent surround resolves to white, so the dab is the only
    // thing that darkens the mask. Green converts by luminance (~182).
    let moved_to = at(48, 48);
    assert!(
        moved_to.abs_diff((0.7152f32 * 255.0).round() as u8) <= 6,
        "the dab must land at the moved-to position (48, 48); the mask reads \
         {moved_to} there"
    );
    assert_eq!(
        at(16, 16),
        255,
        "the dab must not also be at its pre-move position (16, 16)"
    );
}

/// Bug 2: a region copy (selection active) of a masked layer pastes flat, with
/// no mask attached.
#[test]
fn region_copy_of_masked_layer_pastes_without_mask() {
    let (w, h) = (32u32, 32u32);
    let mut source = test_engine(w, h);
    let layer = source.add_raster_layer(None);
    paint_dot(&mut source, layer, 16.0, 16.0, (1.0, 0.0, 0.0));
    source.add_mask(layer);

    // Active selection ⇒ this is a region copy, not a whole-layer copy.
    source.select_rect(0.0, 0.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    settle(&mut source);

    source.copy_layer_rich(layer);
    let json = drain_rich_copy(&mut source);

    let mut sink = test_engine(w, h);
    let pasted = sink.paste_layer_rich(&json, None).expect("paste succeeds");

    let (_, _, _, modifier_count) = raster_props(&sink, pasted);
    assert_eq!(
        modifier_count, 0,
        "a region copy must not carry a mask onto the pasted layer"
    );
}

/// Bug 3: pasting a masked layer must not seed the restored mask from the
/// receiving document's active selection.
#[test]
fn paste_does_not_seed_mask_from_active_selection() {
    let (w, h) = (32u32, 32u32);

    // Whole-layer copy (no selection) — the mask travels with the layer.
    let mut source = test_engine(w, h);
    let layer = source.add_raster_layer(None);
    paint_dot(&mut source, layer, 16.0, 16.0, (1.0, 0.0, 0.0));
    source.add_mask(layer);
    source.copy_layer_rich(layer);
    let json = drain_rich_copy(&mut source);
    assert!(
        json.contains("\"mask\":{"),
        "mask metadata must be in the JSON"
    );

    // Receiving document has an active selection when the paste happens.
    let mut sink = test_engine(w, h);
    sink.select_rect(0.0, 0.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    settle(&mut sink);

    let pasted = sink.paste_layer_rich(&json, None).expect("paste succeeds");
    settle(&mut sink);

    // A mask seeded from the half-canvas selection would be non-uniform
    // (255 inside, 0 outside). The restored mask must be uniform (unseeded).
    let m = sink.test_readback_mask(pasted);
    let first = m[0];
    assert!(
        m.iter().all(|&v| v == first),
        "the restored mask must be uniform — paste must not seed it from the \
         active selection"
    );
}
