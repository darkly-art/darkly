//! Canvas flip / rotate + layer / selection flip integration tests.
//!
//! These exercise the exact (no-resample) ortho transform: a per-pixel value
//! grid is permuted and checked **bit-exact**, on **odd** width AND height
//! (where an off-by-one in the `(W-1-i)` index map or the pivot would surface),
//! including offset/smaller-than-canvas layers and a non-rectangular selection.
//! Covers undo/redo round-trips and the selection carry/clear behaviour.
//!
//! Run with: `cargo test -p darkly --test flip_rotate --features testing -- --test-threads=1`

use darkly::document::SelectionMode;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::ortho_transform::OrthoXform;
use darkly::gpu::test_utils::*;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// A `w`×`h` RGBA buffer with a distinct value per pixel: `r = x*10`, `g =
/// y*10` (so every texel is identifiable after a permutation). Dims stay small
/// enough that `x*10`/`y*10` fit in a byte.
fn distinct_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            v[i] = (x * 10) as u8;
            v[i + 1] = (y * 10) as u8;
            v[i + 2] = 0;
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

// ---- Canvas flip (dimension-preserving) ------------------------------------

#[test]
fn flip_canvas_h_mirrors_pixels_odd_dims() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    e.transform_canvas(OrthoXform::FlipH);
    assert_eq!(e.canvas_dimensions(), (w, h), "flip keeps dims");

    let after = e.test_readback_layer(layer);
    for y in 0..h {
        for x in 0..w {
            assert_eq!(
                px(&after, w, x, y),
                px(&before, w, w - 1 - x, y),
                "flip-H: ({x},{y}) should hold the mirror of ({},{y})",
                w - 1 - x
            );
        }
    }
}

#[test]
fn flip_canvas_v_mirrors_pixels_odd_dims() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    e.transform_canvas(OrthoXform::FlipV);
    let after = e.test_readback_layer(layer);
    for y in 0..h {
        for x in 0..w {
            assert_eq!(px(&after, w, x, y), px(&before, w, x, h - 1 - y));
        }
    }
}

#[test]
fn rotate_canvas_180_equals_flip_h_then_v() {
    let (w, h) = (7u32, 5u32);
    let mut a = test_engine(w, h);
    let la = a.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    a.transform_canvas(OrthoXform::Rot180);

    let mut b = test_engine(w, h);
    let lb = b.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    b.transform_canvas(OrthoXform::FlipH);
    b.transform_canvas(OrthoXform::FlipV);

    assert_eq!(a.canvas_dimensions(), (w, h));
    assert_eq!(
        a.test_readback_layer(la),
        b.test_readback_layer(lb),
        "rot180 must equal flipH∘flipV"
    );
}

// ---- Canvas rotate (dimension-swapping) ------------------------------------

#[test]
fn rotate_canvas_cw_swaps_dims_and_maps_pixels() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);

    e.transform_canvas(OrthoXform::Rot90Cw);
    assert_eq!(e.canvas_dimensions(), (h, w), "CW swaps dims to h×w");

    // Destination is h-wide, w-tall; dest(dx,dy) ← orig(dy, h-1-dx).
    let after = e.test_readback_layer(layer);
    for dy in 0..w {
        for dx in 0..h {
            let expect = [(dy * 10) as u8, ((h - 1 - dx) * 10) as u8, 0, 255];
            assert_eq!(px(&after, h, dx, dy), expect, "CW map at ({dx},{dy})");
        }
    }
}

#[test]
fn rotate_canvas_ccw_swaps_dims_and_maps_pixels() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);

    e.transform_canvas(OrthoXform::Rot90Ccw);
    assert_eq!(e.canvas_dimensions(), (h, w));

    // CCW: dest(dx,dy) ← orig(w-1-dy, dx).
    let after = e.test_readback_layer(layer);
    for dy in 0..w {
        for dx in 0..h {
            let expect = [((w - 1 - dy) * 10) as u8, (dx * 10) as u8, 0, 255];
            assert_eq!(px(&after, h, dx, dy), expect, "CCW map at ({dx},{dy})");
        }
    }
}

#[test]
fn rotate_canvas_cw_four_times_is_identity() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    for _ in 0..4 {
        e.transform_canvas(OrthoXform::Rot90Cw);
    }
    assert_eq!(
        e.canvas_dimensions(),
        (w, h),
        "4×CW returns to original dims"
    );
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "4×90° CW must be the identity"
    );
}

// ---- Canvas transform undo / redo ------------------------------------------

#[test]
fn flip_canvas_undo_redo_round_trips_pixels() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    e.transform_canvas(OrthoXform::FlipH);
    let flipped = e.test_readback_layer(layer);

    e.undo();
    assert_eq!(e.canvas_dimensions(), (w, h));
    assert_eq!(e.test_readback_layer(layer), before, "undo restores pixels");

    e.redo();
    assert_eq!(
        e.test_readback_layer(layer),
        flipped,
        "redo restores the flip"
    );
}

#[test]
fn rotate_canvas_cw_undo_restores_dims_and_pixels() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    e.transform_canvas(OrthoXform::Rot90Cw);
    assert_eq!(e.canvas_dimensions(), (h, w));

    e.undo();
    assert_eq!(e.canvas_dimensions(), (w, h), "undo restores swapped dims");
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "undo restores pre-rotate pixels exactly"
    );
}

// ---- Canvas transform + selection ------------------------------------------

#[test]
fn flip_canvas_carries_selection() {
    let (w, h) = (16u32, 16u32);
    let mut e = test_engine(w, h);
    let _l = e.add_raster_layer(None);
    e.select_rect(2.0, 2.0, 6.0, 6.0, SelectionMode::Replace, false, 0.0);
    assert!(e.has_selection());

    e.transform_canvas(OrthoXform::FlipH);
    assert!(
        e.has_selection(),
        "a dimension-preserving canvas flip carries the selection"
    );

    e.undo();
    assert!(e.has_selection(), "undo keeps the selection");
}

#[test]
fn rotate_canvas_clears_selection() {
    let (w, h) = (16u32, 16u32);
    let mut e = test_engine(w, h);
    let _l = e.add_raster_layer(None);
    e.select_rect(2.0, 2.0, 6.0, 6.0, SelectionMode::Replace, false, 0.0);
    assert!(e.has_selection());

    e.transform_canvas(OrthoXform::Rot90Cw);
    assert!(
        !e.has_selection(),
        "a dimension-swapping rotate clears the selection (folded into undo)"
    );

    e.undo();
    assert!(e.has_selection(), "undo restores the cleared selection");
}

// ---- Layer flip (no selection) ---------------------------------------------

#[test]
fn flip_layer_h_no_selection_mirrors_whole_layer() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    assert!(e.flip_node(layer, OrthoXform::FlipH));
    let after = e.test_readback_layer(layer);
    for y in 0..h {
        for x in 0..w {
            assert_eq!(px(&after, w, x, y), px(&before, w, w - 1 - x, y));
        }
    }

    e.undo();
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "undo restores the layer"
    );
}

#[test]
fn flip_layer_offset_layer_mirrors_about_own_centre() {
    // A 4×4 layer placed off-origin on a larger canvas flips within itself.
    let (cw, ch) = (10u32, 8u32);
    let (lw, lh) = (4u32, 4u32);
    let mut e = test_engine(cw, ch);
    let layer = e.paste_image(lw, lh, &distinct_rgba(lw, lh), 2, 1, None);
    let before = e.test_readback_layer(layer);
    assert_eq!(
        before.len(),
        (lw * lh * 4) as usize,
        "layer keeps its 4×4 extent"
    );

    assert!(e.flip_node(layer, OrthoXform::FlipH));
    let after = e.test_readback_layer(layer);
    for y in 0..lh {
        for x in 0..lw {
            assert_eq!(px(&after, lw, x, y), px(&before, lw, lw - 1 - x, y));
        }
    }
}

// ---- Layer flip with a rectangular selection -------------------------------

#[test]
fn flip_layer_with_rect_selection_only_flips_selection() {
    let (w, h) = (12u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    // Odd-width selection x∈[2,7): plane x maps i→4-i about the bbox centre,
    // i.e. plane x' = 6 - x; the centre column x=4 is fixed.
    e.select_rect(2.0, 2.0, 5.0, 5.0, SelectionMode::Replace, false, 0.0);
    assert!(e.flip_node(layer, OrthoXform::FlipH));
    let after = e.test_readback_layer(layer);

    // Inside the selection: mirrored about the bbox centre.
    assert_eq!(
        px(&after, w, 2, 3),
        px(&before, w, 6, 3),
        "left edge ↔ right edge"
    );
    assert_eq!(px(&after, w, 6, 3), px(&before, w, 2, 3));
    assert_eq!(
        px(&after, w, 4, 3),
        px(&before, w, 4, 3),
        "centre column fixed"
    );
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

// ---- Layer flip with a non-rectangular selection ---------------------------

#[test]
fn flip_layer_with_ellipse_selection_clips_to_shape() {
    let (w, h) = (12u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    // Ellipse in bbox x,y ∈ [2,10): centre (6,6), radius 4.
    e.select_ellipse(2.0, 2.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    assert!(e.flip_node(layer, OrthoXform::FlipH));
    let after = e.test_readback_layer(layer);

    // A bbox corner is outside the ellipse → untouched (shape-masked, not bbox).
    assert_eq!(
        px(&after, w, 2, 2),
        px(&before, w, 2, 2),
        "bbox corner outside the ellipse must be untouched"
    );
    // Near the centre row, well inside the ellipse → mirrored (plane x' = 11-x).
    assert_eq!(px(&after, w, 5, 6), px(&before, w, 6, 6));
    assert_eq!(px(&after, w, 6, 6), px(&before, w, 5, 6));

    e.undo();
    assert_eq!(e.test_readback_layer(layer), before);
}
