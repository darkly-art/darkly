//! Regression tests for the copy/paste ↔ mask interaction.
//!
//! Three defects are covered:
//!   1. Pasting while a mask is the active edit target must write the clipboard
//!      pixels INTO the mask's R8 texture, not spawn a new sibling layer.
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

fn paint_dot(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32) {
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
        cr: 1.0,
        cg: 0.0,
        cb: 0.0,
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

/// A `pw`×`ph` RGBA buffer whose red channel (64) differs from its luminance
/// (≈140). Pins the RGBA→R8 conversion to the red channel: a luminance
/// implementation would write ≈140 into the mask, not 64.
fn red_distinct_buffer(pw: u32, ph: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 64; // R  — the value that must land in the mask
        px[1] = 200; // G
        px[2] = 32; // B
        px[3] = 255; // A
    }
    rgba
}

/// Bug 1: pasting onto a mask edit target writes the mask, not a new layer.
#[test]
fn paste_into_active_mask_writes_mask_pixels_not_new_layer() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.test_mask_id(host).expect("host has a mask filter");
    settle(&mut e);

    let layers_before = e.layer_tree().len();

    let (pw, ph) = (16u32, 16u32);
    let rgba = red_distinct_buffer(pw, ph);
    // Active target is the mask filter id — the mask-editing state.
    let returned = e.paste_image(pw, ph, &rgba, 8, 8, Some(mask));

    assert_eq!(
        returned, mask,
        "paste into a mask must return the mask id, not a freshly created layer"
    );
    assert_eq!(
        e.layer_tree().len(),
        layers_before,
        "pasting into a mask must not create a new layer"
    );

    settle(&mut e);
    let m = e.test_readback_mask(host);
    assert!(
        m.contains(&64),
        "the mask must contain the pasted red-channel value (64)"
    );
    assert!(
        !m.contains(&140),
        "the mask must not contain a luminance-converted value (~140); \
         conversion must take the red channel to match transform_commit.wgsl"
    );

    // Undoable: the paste reverts the mask to its pre-paste pixels.
    e.undo();
    settle(&mut e);
    let m = e.test_readback_mask(host);
    assert!(
        !m.contains(&64),
        "undo must remove the pasted pixels from the mask"
    );
}

/// Bug 1 (grow case): a paste larger than the mask grows it, and undo restores
/// both the pixels and the mask's bounds — proving the compound
/// `PixelBoundsAction + GpuRegionAction`, not a bare region undo.
#[test]
fn paste_into_mask_grows_and_undo_restores_bounds() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);
    let host = e.add_raster_layer(None);
    e.add_mask(host);
    let mask = e.test_mask_id(host).expect("host has a mask filter");
    settle(&mut e);

    // The mask texture's byte count tracks its extent (R8 = 1 byte/px).
    let len_before = e.test_readback_mask(host).len();

    // Paste a rect that extends past the mask's extent, forcing a grow.
    let (pw, ph) = (32u32, 32u32);
    let rgba = red_distinct_buffer(pw, ph);
    e.paste_image(pw, ph, &rgba, (w as i32) - 8, (h as i32) - 8, Some(mask));
    settle(&mut e);

    let len_grown = e.test_readback_mask(host).len();
    assert!(
        len_grown > len_before,
        "a paste beyond the mask extent must grow the mask texture \
         (before={len_before}, grown={len_grown})"
    );

    e.undo();
    settle(&mut e);
    let len_after = e.test_readback_mask(host).len();
    assert_eq!(
        len_after, len_before,
        "undo must restore the mask's bounds (texture size), not just its pixels"
    );
}

/// Bug 2: a region copy (selection active) of a masked layer pastes flat, with
/// no mask attached.
#[test]
fn region_copy_of_masked_layer_pastes_without_mask() {
    let (w, h) = (32u32, 32u32);
    let mut source = test_engine(w, h);
    let layer = source.add_raster_layer(None);
    paint_dot(&mut source, layer, 16.0, 16.0);
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
    paint_dot(&mut source, layer, 16.0, 16.0);
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
