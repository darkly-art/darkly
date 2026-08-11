//! Regression tests for the copy/paste ↔ mask interaction.
//!
//! Model: plain paste always makes its own layer; pasting INTO the active
//! target (a layer or a mask) is the "paste in place" verb, which floats the
//! clip onto the active node and commits through the shared `commit_floating`
//! path (RGBA layers and R8 masks alike). The three defects covered:
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

/// Bug 1 (floating verb): paste-in-place floats the clipboard onto the active
/// MASK and commits into it — no new layer, and undoable. This is the path the
/// UI takes when "activate transform after paste" is on.
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
