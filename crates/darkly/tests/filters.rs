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
use darkly::engine::types::{LayerInfo, StrokeOp};
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::params::ParamValue;
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

// ---- Filter layers ---------------------------------------------------------
//
// A *filter layer* is a non-destructive node in the layer tree that transforms
// the composite of everything below it (the running group accumulator) via the
// same `gpu/filters/*` pipeline the destructive path uses — pixels below are
// never modified. These tests pin the feature's promises: it inverts what's
// below it, it leaves what's above untouched, an isolated group scopes it, and
// — the core guarantee — it is non-destructive (toggle / delete restores the
// original composite byte-for-byte).

/// Flood-fill a layer with straight opaque `(r, g, b, 255)`. Opaque so the
/// composite reads back as the layer color (no premultiply / checker ambiguity)
/// and `invert` is unambiguous: `(r,g,b)` → `(255-r, 255-g, 255-b)`.
fn fill_layer(engine: &mut DarklyEngine, layer_id: LayerId, r: u8, g: u8, b: u8) {
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r,
        g,
        b,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

/// A filter layer above a raster at the root inverts everything below it:
/// red `(255,0,0)` → cyan `(0,255,255)`. This is the core scope plus
/// "filter layer at root affects all below."
#[test]
fn filter_layer_at_root_inverts_everything_below() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);
    let _filter = engine
        .add_filter_layer("invert", vec![], None)
        .expect("invert is a registered filter type");
    engine.test_flush_readbacks();
    engine.render(0.0);

    let canvas = engine.test_readback_canvas();
    let p = px(&canvas, cw, cw / 2, ch / 2);
    assert_eq!(
        p,
        [0, 255, 255, 255],
        "invert filter layer must turn the red layer below it cyan; got {p:?}"
    );
}

/// A filter layer transforms only what is *below* it — a layer stacked above
/// the filter is composited after the filter runs, so it is untouched. Blue
/// `(0,0,255)` on top stays blue (a leak would make it yellow `(255,255,0)`).
#[test]
fn filter_layer_does_not_affect_layers_above_it() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);
    let _filter = engine.add_filter_layer("invert", vec![], None).unwrap();
    let blue = engine.add_raster_layer(None);
    fill_layer(&mut engine, blue, 0, 0, 255);
    engine.test_flush_readbacks();
    engine.render(0.0);

    let canvas = engine.test_readback_canvas();
    let p = px(&canvas, cw, cw / 2, ch / 2);
    assert_eq!(
        p,
        [0, 0, 255, 255],
        "the opaque blue layer above the filter must be unaffected; got {p:?}"
    );
}

/// A filter layer inside a non-passthrough (isolated) group is scoped to that
/// group: it inverts the group's lower siblings, and does NOT leak onto layers
/// below the group. Positive control (group has content → cyan) and negative
/// (group content hidden → the outside layer stays red, no leak) in one setup.
#[test]
fn filter_layer_in_isolated_group_is_scoped() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    // Outside the group, at the root bottom: red. The group stacks above it.
    let outside = engine.add_raster_layer(None);
    fill_layer(&mut engine, outside, 255, 0, 0);

    let group = engine.add_group(None);
    engine.set_group_passthrough(group, false); // isolated → owns a GroupState

    // Group content (bottom child) + filter (top child, above the content).
    let inside = engine.add_raster_layer(Some(group));
    fill_layer(&mut engine, inside, 255, 0, 0);
    let _filter = engine
        .add_filter_layer("invert", vec![], Some(group))
        .unwrap();
    engine.test_flush_readbacks();
    engine.render(0.0);

    // Positive: the group composites inside→invert→cyan and (opaque) covers the
    // outside red.
    let with_content = engine.test_readback_canvas();
    let p = px(&with_content, cw, cw / 2, ch / 2);
    assert_eq!(
        p,
        [0, 255, 255, 255],
        "filter must invert its isolated group's lower sibling; got {p:?}"
    );

    // Negative: hide the group's content. The group accumulator is now empty,
    // so the filter inverts nothing and the group contributes nothing. The
    // outside red layer shows through unchanged — proving the filter did NOT
    // leak out of the isolated group onto the layer below it (a leak would
    // invert outside red → cyan).
    engine.set_layer_visible(inside, false);
    engine.render(0.0);
    let leak_check = engine.test_readback_canvas();
    let p = px(&leak_check, cw, cw / 2, ch / 2);
    assert_eq!(
        p,
        [255, 0, 0, 255],
        "an isolated group's filter must not affect layers below the group; got {p:?}"
    );
}

/// A mask on a filter layer confines *where the filter applies*: inside the
/// mask the inverted result shows; outside, the original pixels pass through;
/// a mid-gray mask value lerps between the two (soft masking, not a hard
/// threshold). This is the adjustment-layer-mask behavior. Fails before the
/// `compose_filter_arm` mask branch — the filter would invert the whole canvas
/// and the right half would read cyan instead of red.
#[test]
fn masked_filter_layer_confines_inversion() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    // Red base; an invert filter above it (cyan where it applies).
    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);
    let filter = engine.add_filter_layer("invert", vec![], None).unwrap();

    // Seed the filter's mask from a left-half selection: left → reveal (1.0,
    // filter applies), right → hide (0.0, original passes through). A selection
    // gives flat, hard-edged regions — no brush feathering to reason about.
    engine.select_rect(
        0.0,
        0.0,
        (cw / 2) as f32,
        ch as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    engine.add_mask(filter);
    let mask = engine.test_mask_id(filter).expect("mask present on filter");
    engine.clear_selection();
    engine.test_flush_readbacks();
    engine.render(0.0);

    let canvas = engine.test_readback_canvas();
    // Left half (mask 1.0): filter applies → cyan.
    assert_eq!(
        px(&canvas, cw, cw / 4, ch / 2),
        [0, 255, 255, 255],
        "masked-in half must invert to cyan",
    );
    // Right half (mask 0.0): original red passes through (the bug shows cyan).
    assert_eq!(
        px(&canvas, cw, 3 * cw / 4, ch / 2),
        [255, 0, 0, 255],
        "masked-out half must keep the original red",
    );

    // Soft masking: paint a mid-gray dab in the (currently hidden) right half;
    // the composite there must lerp between red and cyan, not snap to either.
    paint_dab(&mut engine, mask, (3 * cw / 4) as f32, (ch / 2) as f32, 0.5);
    engine.test_flush_readbacks();
    engine.render(0.0);
    let canvas = engine.test_readback_canvas();
    let p = px(&canvas, cw, 3 * cw / 4, ch / 2);
    assert!(
        p[0] > 40 && p[0] < 215 && p[1] > 40 && p[1] < 215,
        "a mid-gray mask value must lerp red↔cyan (got {p:?}), proving soft masking",
    );
}

/// A masked filter layer *inside* a non-passthrough (isolated) group lerps
/// against the **group's** accumulator, not the canvas: the mask confines the
/// inversion to the group's own content. Left half (mask 1.0) → the group's red
/// inverts to cyan; right half (mask 0.0) → the group's original red shows.
#[test]
fn masked_filter_layer_in_isolated_group_lerps_against_group_accum() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    // A distinct color outside the group, at the root bottom — the group is
    // opaque and covers it, so seeing it anywhere would mean a leak.
    let outside = engine.add_raster_layer(None);
    fill_layer(&mut engine, outside, 0, 255, 0);

    let group = engine.add_group(None);
    engine.set_group_passthrough(group, false); // isolated → owns a GroupState

    let inside = engine.add_raster_layer(Some(group));
    fill_layer(&mut engine, inside, 255, 0, 0);
    let filter = engine
        .add_filter_layer("invert", vec![], Some(group))
        .unwrap();

    // Mask the filter to the left half (seeded from a selection).
    engine.select_rect(
        0.0,
        0.0,
        (cw / 2) as f32,
        ch as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    engine.add_mask(filter);
    engine.clear_selection();
    engine.test_flush_readbacks();
    engine.render(0.0);

    let canvas = engine.test_readback_canvas();
    // Left half: the group's red inverts to cyan within the group.
    assert_eq!(
        px(&canvas, cw, cw / 4, ch / 2),
        [0, 255, 255, 255],
        "masked-in half inverts the group's own content to cyan",
    );
    // Right half: the filter is masked out → the group's original red shows
    // (lerped against the group accumulator, never the green canvas below).
    assert_eq!(
        px(&canvas, cw, 3 * cw / 4, ch / 2),
        [255, 0, 0, 255],
        "masked-out half keeps the group's red, with no canvas leak",
    );
}

/// The core promise: a filter layer is **non-destructive**. Toggling its
/// visibility returns the composite to the original red (the layer below was
/// never modified), and deleting it likewise restores the original — a
/// destructive filter would have baked cyan into the raster's pixels.
#[test]
fn filter_layer_is_non_destructive() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);

    // Baseline composite with no filter: red.
    engine.test_flush_readbacks();
    engine.render(0.0);
    let original = engine.test_readback_canvas();
    assert_eq!(px(&original, cw, cw / 2, ch / 2), [255, 0, 0, 255]);

    let filter = engine.add_filter_layer("invert", vec![], None).unwrap();
    engine.render(0.0);
    assert_eq!(
        px(&engine.test_readback_canvas(), cw, cw / 2, ch / 2),
        [0, 255, 255, 255],
        "filter visible → cyan"
    );

    // Hide the filter → original red returns byte-for-byte (pixels untouched).
    engine.set_layer_visible(filter, false);
    engine.render(0.0);
    assert_eq!(
        engine.test_readback_canvas(),
        original,
        "hiding the filter must restore the original composite exactly"
    );

    // Show again → cyan.
    engine.set_layer_visible(filter, true);
    engine.render(0.0);
    assert_eq!(
        px(&engine.test_readback_canvas(), cw, cw / 2, ch / 2),
        [0, 255, 255, 255],
        "re-showing the filter inverts again"
    );

    // Delete the filter → the composite returns to the original red.
    engine.remove_layer(filter).expect("filter layer removable");
    engine.render(0.0);
    assert_eq!(
        engine.test_readback_canvas(),
        original,
        "deleting the filter must restore the original composite exactly"
    );
}

// ---- Curves (parametric) filter layer --------------------------------------
//
// Curves is the first parametric filter: five per-channel tone curves baked
// into a GPU LUT. These pin the schema (five identity curves on add), the
// param-edit + undo path, the destructive-path exclusion, and the two GPU
// promises (identity ⇒ bit-unchanged; a value curve shifts pixels as baked).

/// An identity curve — a straight diagonal.
fn identity_curve() -> ParamValue {
    ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]])
}

/// The default param vector for a filter type, straight from its schema.
fn filter_defaults(e: &DarklyEngine, type_id: &str) -> Vec<ParamValue> {
    e.filter_param_defs(type_id)
        .iter()
        .map(|d| d.default_value())
        .collect()
}

/// Pull a root filter layer's effective param values (`value`, else `default`)
/// from the engine's layer-tree query.
fn filter_layer_params(e: &DarklyEngine, id: LayerId) -> Vec<ParamValue> {
    let ffi = id.to_ffi() as f64;
    for node in e.layer_tree() {
        if let LayerInfo::Filter {
            id: nid, params, ..
        } = &node
        {
            if *nid == ffi {
                return params
                    .iter()
                    .map(|p| p.value.clone().unwrap_or_else(|| p.default.clone()))
                    .collect();
            }
        }
    }
    panic!("filter layer {ffi} not found in layer tree");
}

#[test]
fn add_curves_layer_yields_five_identity_curves() {
    let mut e = test_engine(8, 8);
    let params = filter_defaults(&e, "curves");
    let id = e
        .add_filter_layer("curves", params, None)
        .expect("curves is a registered filter type");

    let got = filter_layer_params(&e, id);
    assert_eq!(got.len(), 5, "curves exposes red/green/blue/value/alpha");
    for (i, p) in got.iter().enumerate() {
        assert_eq!(
            p,
            &identity_curve(),
            "default curve param {i} must be identity"
        );
    }
}

#[test]
fn update_filter_params_mutates_and_undo_restores() {
    let mut e = test_engine(8, 8);
    let id = e
        .add_filter_layer("curves", filter_defaults(&e, "curves"), None)
        .unwrap();

    // Edit the value curve (index 3) to a non-identity darkening ramp.
    let edited = {
        let mut p = filter_defaults(&e, "curves");
        p[3] = ParamValue::Curve(vec![[0.0, 0.0], [1.0, 0.5]]);
        p
    };
    e.update_filter_params(id, edited.clone());
    assert_eq!(
        filter_layer_params(&e, id),
        edited,
        "update_filter_params must set the new curves"
    );

    e.undo();
    let restored = filter_layer_params(&e, id);
    for (i, p) in restored.iter().enumerate() {
        assert_eq!(
            p,
            &identity_curve(),
            "undo must restore curve {i} to identity"
        );
    }
}

#[test]
fn apply_filter_excludes_parametric_curves() {
    let (w, h) = (4u32, 4u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    assert!(
        !e.apply_filter(layer, "curves"),
        "a parametric filter must be excluded from the destructive one-click path"
    );
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "the excluded destructive curves call must not touch pixels"
    );
}

/// Identity curves ⇒ the composite is bit-unchanged. Validates the
/// `textureLoad(round(v*255))` index convention: LUT[i] == i, so every byte
/// maps to itself.
#[test]
fn curves_identity_leaves_composite_unchanged() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 200, 64, 128);
    engine.test_flush_readbacks();
    engine.render(0.0);
    let before = engine.test_readback_canvas();

    engine
        .add_filter_layer("curves", filter_defaults(&engine, "curves"), None)
        .unwrap();
    engine.test_flush_readbacks();
    engine.render(0.0);

    assert_eq!(
        engine.test_readback_canvas(),
        before,
        "an identity curves layer must not change any pixel"
    );
}

/// A non-identity value curve shifts pixels as baked: `value(x) = x/2` halves
/// every color channel while leaving alpha untouched (the value curve is not
/// applied to alpha).
#[test]
fn curves_value_curve_shifts_pixels() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 255, 0, 0); // opaque red
    engine.test_flush_readbacks();
    engine.render(0.0);

    // Value curve halves the input; red/green/blue identity, alpha identity.
    let params = vec![
        identity_curve(),
        identity_curve(),
        identity_curve(),
        ParamValue::Curve(vec![[0.0, 0.0], [1.0, 0.5]]),
        identity_curve(),
    ];
    engine.add_filter_layer("curves", params, None).unwrap();
    engine.test_flush_readbacks();
    engine.render(0.0);

    let p = px(&engine.test_readback_canvas(), cw, cw / 2, ch / 2);
    // value(red(1.0)) = value(1.0) = 0.5 → ~128; g/b stay 0; alpha stays 255.
    assert!(
        (p[0] as i32 - 128).abs() <= 2,
        "value curve should halve red to ~128, got {p:?}"
    );
    assert_eq!(p[1], 0, "green stays 0");
    assert_eq!(p[2], 0, "blue stays 0");
    assert_eq!(p[3], 255, "alpha untouched by the value curve");
}
