//! Destructive color-filter integration tests — the "Invert Colors"
//! filter over the shared `filter_node_region` substrate.
//!
//! These are **regression** tests for the node-generic invert path: they pin
//! `1 - c` exactly (per-pixel, RGBA8 layer *and* R8 mask — masks ride the same
//! substrate and must not be left half-done), the invert-twice round-trip,
//! undo/redo, and selection clipping (rect on a layer, ellipse shape-clip, and
//! a selection on a *mask* node). A no-op invert would fail the once-checks.
//!
//! Run with: `cargo test -p darkly --test filters --features testing -- --test-threads=1`

use darkly::coord::{CanvasPoint, CanvasRect};
use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// A `w`×`h` RGBA buffer with a distinct, non-128 value per pixel so `255 - c`
/// is unambiguously different from `c` (catches a no-op invert). Channels are
/// staggered across pixels.
fn distinct_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            v[i] = (x * 10 + 3) as u8;
            v[i + 1] = (y * 10 + 7) as u8;
            v[i + 2] = ((x + y) * 5 + 1) as u8;
            v[i + 3] = 255;
        }
    }
    v
}

/// RGBA quad at `(x, y)` in a `stride`-wide buffer.
fn px(buf: &[u8], stride: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * stride + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// Expected invert of an RGBA quad: `1 - rgb`, alpha preserved.
fn inv(p: [u8; 4]) -> [u8; 4] {
    [255 - p[0], 255 - p[1], 255 - p[2], p[3]]
}

/// Paint a single grayscale brush dab onto a node so an R8 mask becomes
/// non-uniform — `value` lands in the mask's R channel.
fn paint_dab(engine: &mut DarklyEngine, node_id: LayerId, x: f32, y: f32, value: f32) {
    engine.begin_stroke(node_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: value,
        cg: value,
        cb: value,
        ca: 1.0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

// ---- Layer (RGBA8) ---------------------------------------------------------

#[test]
fn invert_layer_negates_every_channel() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    assert!(e.apply_filter(layer, "invert"));
    let after = e.test_readback_layer(layer);
    for y in 0..h {
        for x in 0..w {
            assert_eq!(
                px(&after, w, x, y),
                inv(px(&before, w, x, y)),
                "invert: ({x},{y}) should be 255-c with alpha preserved"
            );
        }
    }
}

#[test]
fn invert_layer_twice_is_identity() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    assert!(e.apply_filter(layer, "invert"));
    assert!(e.apply_filter(layer, "invert"));
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "invert twice restores the original pixels exactly"
    );
}

#[test]
fn invert_layer_undo_redo_round_trips() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    assert!(e.apply_filter(layer, "invert"));
    let inverted = e.test_readback_layer(layer);

    e.undo();
    assert_eq!(e.test_readback_layer(layer), before, "undo restores pixels");

    e.redo();
    assert_eq!(
        e.test_readback_layer(layer),
        inverted,
        "redo re-applies the invert"
    );
}

#[test]
fn invert_unknown_type_is_a_noop() {
    let (w, h) = (4u32, 4u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    assert!(
        !e.apply_filter(layer, "no_such_adjustment"),
        "an unregistered type must return false"
    );
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "an unknown filter must not touch pixels"
    );
}

// ---- Layer + selection -----------------------------------------------------

#[test]
fn invert_layer_with_rect_selection_only_inverts_selection() {
    let (w, h) = (12u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    // Rect x,y ∈ [3,8).
    e.select_rect(3.0, 3.0, 5.0, 5.0, SelectionMode::Replace, false, 0.0);
    assert!(e.apply_filter(layer, "invert"));
    let after = e.test_readback_layer(layer);

    // Inside the selection: inverted.
    assert_eq!(px(&after, w, 4, 4), inv(px(&before, w, 4, 4)));
    assert_eq!(px(&after, w, 7, 7), inv(px(&before, w, 7, 7)));
    // Outside the selection: untouched.
    assert_eq!(px(&after, w, 0, 0), px(&before, w, 0, 0));
    assert_eq!(px(&after, w, 10, 10), px(&before, w, 10, 10));

    e.undo();
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "undo restores the layer"
    );
}

#[test]
fn invert_layer_with_ellipse_selection_clips_to_shape() {
    let (w, h) = (12u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    // Ellipse in bbox x,y ∈ [2,10): centre (6,6).
    e.select_ellipse(2.0, 2.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    assert!(e.apply_filter(layer, "invert"));
    let after = e.test_readback_layer(layer);

    // A bbox corner is outside the ellipse → untouched (shape-masked, not bbox).
    assert_eq!(
        px(&after, w, 2, 2),
        px(&before, w, 2, 2),
        "bbox corner outside the ellipse must be untouched"
    );
    // The centre is well inside the ellipse → inverted.
    assert_eq!(px(&after, w, 6, 6), inv(px(&before, w, 6, 6)));

    e.undo();
    assert_eq!(e.test_readback_layer(layer), before);
}

// ---- Mask (R8) — guards the node-generic path ------------------------------

#[test]
fn invert_mask_negates_r8_and_round_trips() {
    let (w, h) = (16u32, 16u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);
    e.add_mask(layer);
    let mask = e.test_mask_id(layer).expect("mask present");

    // Make the mask non-uniform: a black dab on the default-white mask.
    paint_dab(&mut e, mask, (w / 2) as f32, (h / 2) as f32, 0.0);
    let before = e.test_readback_layer(mask);
    assert_eq!(
        before.len(),
        (w * h) as usize,
        "mask is R8 — one byte/pixel"
    );

    assert!(e.apply_filter(mask, "invert"));
    let after = e.test_readback_layer(mask);
    for i in 0..before.len() {
        assert_eq!(after[i], 255 - before[i], "mask byte {i} should be 1-r");
    }

    assert!(e.apply_filter(mask, "invert"));
    assert_eq!(
        e.test_readback_layer(mask),
        before,
        "invert twice restores the mask exactly"
    );
}

#[test]
fn invert_mask_with_selection_only_inverts_selected_region() {
    let (w, h) = (12u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);
    e.add_mask(layer);
    let mask = e.test_mask_id(layer).expect("mask present");
    let before = e.test_readback_layer(mask);

    // Rect x,y ∈ [3,8) — only this region of the mask inverts.
    e.select_rect(3.0, 3.0, 5.0, 5.0, SelectionMode::Replace, false, 0.0);
    assert!(e.apply_filter(mask, "invert"));
    let after = e.test_readback_layer(mask);

    let at = |buf: &[u8], x: u32, y: u32| buf[(y * w + x) as usize];
    // Inside the selection: inverted.
    assert_eq!(at(&after, 4, 4), 255 - at(&before, 4, 4));
    assert_eq!(at(&after, 7, 7), 255 - at(&before, 7, 7));
    // Outside the selection: untouched.
    assert_eq!(at(&after, 0, 0), at(&before, 0, 0));
    assert_eq!(at(&after, 10, 10), at(&before, 10, 10));

    e.undo();
    assert_eq!(
        e.test_readback_layer(mask),
        before,
        "undo restores the mask"
    );
}

// ---- Selection × coordinate frames (crop / rescale) ------------------------
//
// The single most recurring bug class here is carrying a value into the wrong
// coordinate frame (see docs/coordinate-systems.md). `apply_filter` takes
// the window-local selection bbox → plane (`to_canvas(canvas_origin)`) → node-
// local, exactly as `flip_node` does; these intermix a selection with a crop
// (non-zero `canvas_origin`) and a rescale (changed dims) to pin that it lands
// on the right pixels and not offset by the origin or the scale.

#[test]
fn invert_layer_with_selection_after_crop_uses_plane_coords() {
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    // Crop to a non-(0,0) window: origin (8,8), 16×16 → plane window [8,24)².
    // The layer keeps its full 32² extent (crop moves the window, not pixels),
    // so `test_readback_layer` is still 32-wide and indexed in plane coords.
    e.resize_canvas(CanvasRect::new(CanvasPoint::new(8, 8), 16, 16));

    // Selection input is plane-space: plane rect [10,18)². `select_rect`
    // shifts it to window-local [2,10)²; `apply_filter` must shift it back
    // to plane [10,18)² via `canvas_origin` before touching pixels.
    e.select_rect(10.0, 10.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    assert!(e.apply_filter(layer, "invert"));
    let after = e.test_readback_layer(layer);

    // Inside the selected PLANE region — inverted.
    assert_eq!(px(&after, w, 12, 12), inv(px(&before, w, 12, 12)));
    assert_eq!(px(&after, w, 17, 17), inv(px(&before, w, 17, 17)));
    // Outside it — untouched.
    assert_eq!(px(&after, w, 2, 2), px(&before, w, 2, 2));
    assert_eq!(px(&after, w, 25, 25), px(&before, w, 25, 25));
    // Window-local (2,2) is plane (10,10) — already covered above. The mirror
    // guard: plane (18,18) is window-local (10,10), one past the selection's
    // far edge, so it must NOT invert. A missing `to_canvas` shift (treating
    // window-local [2,10) as plane) would invert here and skip (12,12).
    assert_eq!(px(&after, w, 18, 18), px(&before, w, 18, 18));

    e.undo();
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "undo restores the layer"
    );
}

#[test]
fn invert_layer_with_selection_after_rescale() {
    let (w, h) = (16u32, 16u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);

    // Content-scaling resize to 2× — layer pixels are resampled to the new dims.
    e.rescale_image(2 * w, 2 * h);
    let nw = 2 * w;
    let before = e.test_readback_layer(layer);
    assert_eq!(
        before.len(),
        (nw * 2 * h * 4) as usize,
        "layer must be resampled to the doubled dims"
    );

    // Select a plane rect in the rescaled (origin-(0,0)) doc: plane [8,24)².
    e.select_rect(8.0, 8.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    assert!(e.apply_filter(layer, "invert"));
    let after = e.test_readback_layer(layer);

    // Inside the selection — inverted; outside — untouched. Confirms the bbox
    // tracks the post-rescale dims rather than the original 16².
    assert_eq!(px(&after, nw, 12, 12), inv(px(&before, nw, 12, 12)));
    assert_eq!(px(&after, nw, 20, 20), inv(px(&before, nw, 20, 20)));
    assert_eq!(px(&after, nw, 2, 2), px(&before, nw, 2, 2));
    assert_eq!(px(&after, nw, 30, 30), px(&before, nw, 30, 30));

    e.undo();
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "undo restores the rescaled layer"
    );
}

#[test]
fn invert_mask_with_selection_after_crop() {
    // The node-generic path under a non-zero origin: a mask (R8) selection
    // invert after a crop must clip to the same plane region a layer would. A
    // layer mask keeps its full 32² extent at origin (0,0) across a crop (the
    // crop moves only the window), so readbacks are plane-indexed, stride 32.
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);
    e.add_mask(layer);
    let mask = e.test_mask_id(layer).expect("mask present");

    // Non-uniform mask so the selection clip is meaningful.
    paint_dab(&mut e, mask, 14.0, 14.0, 0.0);

    e.resize_canvas(CanvasRect::new(CanvasPoint::new(8, 8), 16, 16));
    let before = e.test_readback_layer(mask);
    assert_eq!(
        before.len(),
        (w * h) as usize,
        "mask keeps its full extent across the crop (R8, one byte/pixel)"
    );

    // Plane selection [10,18)² — same plane region a layer would invert.
    e.select_rect(10.0, 10.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    assert!(e.apply_filter(mask, "invert"));
    let after = e.test_readback_layer(mask);

    let at = |buf: &[u8], x: u32, y: u32| buf[(y * w + x) as usize];
    // Inside plane [10,18)² — inverted.
    assert_eq!(at(&after, 12, 12), 255 - at(&before, 12, 12));
    assert_eq!(at(&after, 17, 17), 255 - at(&before, 17, 17));
    // Outside — untouched. (18,18) is one past the far edge (a missing
    // `to_canvas` shift would inflate the region here).
    assert_eq!(at(&after, 2, 2), at(&before, 2, 2));
    assert_eq!(at(&after, 18, 18), at(&before, 18, 18));

    e.undo();
    assert_eq!(
        e.test_readback_layer(mask),
        before,
        "undo restores the mask"
    );
}
