//! Canvas flip / rotate + layer / selection flip integration tests.
//!
//! These exercise the exact (no-resample) ortho transform: a per-pixel value
//! grid is permuted and checked **bit-exact**, on **odd** width AND height
//! (where an off-by-one in the `(W-1-i)` index map or the pivot would surface),
//! including offset/smaller-than-canvas layers and a non-rectangular selection.
//! Covers undo/redo round-trips and the selection carry/clear behaviour.
//!
//! A second half is a **composition suite**: flip/rotate combined with painting,
//! cropping (non-zero `canvas_origin`), document rescale, masks, multiple layers,
//! deep round-trips, and long undo/redo sequences — the coordinate frames that
//! a transform can quietly corrupt (cf. the `image_rescale.rs` tool-accuracy
//! suite). The crop cases are the ones that exercise a non-`(0,0)` origin.
//!
//! Run with: `cargo test -p darkly --test flip_rotate --features testing -- --test-threads=1`

use darkly::coord::{CanvasPoint, CanvasRect};
use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::ortho_transform::OrthoXform;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

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

// ===========================================================================
// Composition suite — flip/rotate combined with paint, crop, rescale, masks,
// multiple layers, deep round-trips, and long undo/redo sequences.
// ===========================================================================

/// Paint a short red horizontal stroke centred near plane `(cx, cy)`.
fn paint_feature(engine: &mut DarklyEngine, layer_id: LayerId, cx: f32, cy: f32) {
    engine.begin_stroke(layer_id);
    let steps = 16;
    for i in 0..=steps {
        let x = cx - 3.0 + 6.0 * (i as f32 / steps as f32);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: cy,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
}

/// Centroid (in canvas px) of clearly-red pixels in an RGBA8 buffer.
fn red_centroid(px: &[u8], w: u32, h: u32) -> (f32, f32) {
    let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0u32);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if px[i] > 128 && px[i + 1] < 80 && px[i + 2] < 80 {
                sx += x as f32;
                sy += y as f32;
                n += 1;
            }
        }
    }
    assert!(n > 0, "no red pixels found");
    (sx / n as f32, sy / n as f32)
}

/// Alpha at plane `(x, y)` of a `stride`-wide RGBA readback.
fn alpha_at(px: &[u8], stride: u32, x: u32, y: u32) -> u8 {
    px[((y * stride + x) * 4 + 3) as usize]
}

/// Drive pending async GPU readbacks to completion, `n` frames.
fn pump(engine: &mut DarklyEngine, n: u32) {
    for _ in 0..n {
        engine.test_flush_readbacks();
        engine.render(0.0);
    }
}

/// A transparent `cw`×`ch` RGBA buffer with an opaque red `sz`×`sz` square at
/// plane `(x0, y0)`. A deterministic position marker — unlike the default brush
/// (which floods small canvases), a pasted square has an exact, clip-free
/// centroid, so it's the right tool for absolute-position assertions.
fn rgba_with_marker(cw: u32, ch: u32, x0: u32, y0: u32, sz: u32) -> Vec<u8> {
    let mut v = vec![0u8; (cw * ch * 4) as usize];
    for y in y0..(y0 + sz).min(ch) {
        for x in x0..(x0 + sz).min(cw) {
            let i = ((y * cw + x) * 4) as usize;
            v[i] = 255;
            v[i + 3] = 255;
        }
    }
    v
}

// ---- Round-trip identities (pixel-exact) -----------------------------------

#[test]
fn flip_twice_is_identity() {
    let (w, h) = (7u32, 5u32);
    for axis in [OrthoXform::FlipH, OrthoXform::FlipV] {
        let mut e = test_engine(w, h);
        let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
        let before = e.test_readback_layer(layer);
        e.transform_canvas(axis);
        e.transform_canvas(axis);
        assert_eq!(e.canvas_dimensions(), (w, h));
        assert_eq!(
            e.test_readback_layer(layer),
            before,
            "{axis:?} twice = identity"
        );
    }
}

#[test]
fn rotate_cw_then_ccw_is_identity() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);
    e.transform_canvas(OrthoXform::Rot90Cw);
    e.transform_canvas(OrthoXform::Rot90Ccw);
    assert_eq!(e.canvas_dimensions(), (w, h));
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "CW then CCW = identity"
    );
}

#[test]
fn rotate_180_twice_is_identity() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);
    e.transform_canvas(OrthoXform::Rot180);
    e.transform_canvas(OrthoXform::Rot180);
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "rot180 twice = identity"
    );
}

// ---- Multiple layers + a mask, transformed together ------------------------

#[test]
fn flip_canvas_transforms_every_layer_and_undo_restores_all() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let l1 = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    // A second, differently-valued layer so the two can't be confused.
    let mut rgba2 = distinct_rgba(w, h);
    for p in rgba2.chunks_exact_mut(4) {
        p[2] = 200; // tag layer 2 with a blue channel
    }
    let l2 = e.paste_image(w, h, &rgba2, 0, 0, None);

    let b1 = e.test_readback_layer(l1);
    let b2 = e.test_readback_layer(l2);

    e.transform_canvas(OrthoXform::FlipH);
    for (layer, before) in [(l1, &b1), (l2, &b2)] {
        let after = e.test_readback_layer(layer);
        for y in 0..h {
            for x in 0..w {
                assert_eq!(px(&after, w, x, y), px(before, w, w - 1 - x, y));
            }
        }
    }

    e.undo();
    assert_eq!(e.test_readback_layer(l1), b1, "layer 1 restored");
    assert_eq!(e.test_readback_layer(l2), b2, "layer 2 restored");
}

#[test]
fn rotate_canvas_carries_layer_mask_in_lockstep() {
    let (w, h) = (8u32, 6u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);
    e.add_mask(layer);
    let mask = e.test_mask_id(layer).expect("mask present");
    assert_eq!(
        e.test_node_pixel_bounds(mask),
        Some(CanvasRect::from_xywh(0, 0, w, h))
    );

    e.transform_canvas(OrthoXform::Rot90Cw);
    // Canvas swapped to h×w; the mask extent must swap with it.
    assert_eq!(e.canvas_dimensions(), (h, w));
    assert_eq!(
        e.test_node_pixel_bounds(mask),
        Some(e.canvas_rect()),
        "mask extent must track the rotated canvas window"
    );
    assert_eq!(
        e.test_readback_layer(mask).len(),
        (h * w) as usize,
        "mask R8 texture must be reallocated to the swapped dims"
    );

    e.undo();
    assert_eq!(e.canvas_dimensions(), (w, h));
    assert_eq!(
        e.test_node_pixel_bounds(mask),
        Some(CanvasRect::from_xywh(0, 0, w, h))
    );
}

// ---- Non-zero canvas_origin (crop → flip / rotate) -------------------------

#[test]
fn flip_canvas_after_crop_mirrors_about_the_window() {
    // A 4×4 marker at plane [6,10) on 32², cropped to window [4,20)² (origin
    // (4,4)): its window-local centroid is 3.5. flipH mirrors it about the
    // WINDOW centre to local x = 15-3.5 = 11.5. Exercises a non-(0,0)
    // canvas_origin with a layer larger than the window. A pasted marker (not a
    // brush) keeps the centroid exact and clip-free.
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let _layer = e.paste_image(w, h, &rgba_with_marker(w, h, 6, 6, 4), 0, 0, None);

    e.resize_canvas(CanvasRect::new(CanvasPoint::new(4, 4), 16, 16));
    let before = e.test_readback_canvas();
    let (cx0, cy0) = red_centroid(&before, 16, 16);
    assert!(
        (cx0 - 3.5).abs() < 1.0 && (cy0 - 3.5).abs() < 1.0,
        "pre-flip marker should sit at window-local ~(3.5,3.5), got ({cx0},{cy0})"
    );

    e.transform_canvas(OrthoXform::FlipH);
    assert_eq!(
        e.canvas_rect(),
        CanvasRect::new(CanvasPoint::new(4, 4), 16, 16)
    );
    let after = e.test_readback_canvas();
    let (cx1, cy1) = red_centroid(&after, 16, 16);
    assert!(
        (cx1 - 11.5).abs() < 1.0 && (cy1 - 3.5).abs() < 1.0,
        "post-flip marker should mirror to window-local ~(11.5,3.5), got ({cx1},{cy1})"
    );
}

#[test]
fn rotate_after_crop_round_trips_origin_dims_and_pixels() {
    // A non-square crop (origin (4,4), 16×12) round-trips through CW∘CCW with
    // the recentred origin landing back exactly, and the visible pixels intact.
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);
    paint_feature(&mut e, layer, 10.0, 10.0);

    let crop = CanvasRect::new(CanvasPoint::new(4, 4), 16, 12);
    e.resize_canvas(crop);
    let before = e.test_readback_canvas();

    e.transform_canvas(OrthoXform::Rot90Cw);
    assert_eq!(
        e.canvas_dimensions(),
        (12, 16),
        "CW swaps the cropped window dims"
    );
    e.transform_canvas(OrthoXform::Rot90Ccw);

    assert_eq!(
        e.canvas_rect(),
        crop,
        "CW∘CCW restores origin + dims exactly"
    );
    assert_eq!(
        e.test_readback_canvas(),
        before,
        "CW∘CCW restores the visible pixels exactly"
    );
}

#[test]
fn crop_then_flip_undo_restores_full_canvas() {
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);
    paint_feature(&mut e, layer, 12.0, 9.0);
    let original = e.test_readback_layer(layer);

    e.resize_canvas(CanvasRect::new(CanvasPoint::new(4, 4), 16, 16));
    e.transform_canvas(OrthoXform::FlipH);
    e.undo(); // undo the flip
    e.undo(); // undo the crop

    assert_eq!(e.canvas_rect(), CanvasRect::from_xywh(0, 0, w, h));
    assert_eq!(
        e.test_readback_layer(layer),
        original,
        "undoing flip then crop restores the original layer pixels"
    );
}

// ---- Composition with document rescale -------------------------------------

#[test]
fn rescale_then_flip_mirrors_the_scaled_feature() {
    // A 4×4 marker at plane [8,12) (centroid 10) scales ×2 about the origin to
    // centroid 20 on the 64² canvas, then flipH mirrors it to 63-20 = 43.
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let _layer = e.paste_image(w, h, &rgba_with_marker(w, h, 8, 8, 4), 0, 0, None);

    e.rescale_image(2 * w, 2 * h);
    e.transform_canvas(OrthoXform::FlipH);

    let after = e.test_readback_canvas();
    let (cx, cy) = red_centroid(&after, 2 * w, 2 * h);
    assert!(
        (cx - 43.0).abs() < 2.5 && (cy - 20.0).abs() < 2.5,
        "scaled-then-flipped marker should be ~(43,20), got ({cx},{cy})"
    );
}

#[test]
fn flip_then_rescale_undo_undo_restores_original() {
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);
    paint_feature(&mut e, layer, 10.0, 12.0);
    let original = e.test_readback_layer(layer);

    e.transform_canvas(OrthoXform::FlipH);
    e.rescale_image(2 * w, 2 * h);
    e.undo(); // undo rescale
    e.undo(); // undo flip

    assert_eq!(e.canvas_dimensions(), (w, h));
    assert_eq!(
        e.test_readback_layer(layer),
        original,
        "undo(rescale) then undo(flip) restores the original pixels"
    );
}

// ---- Painting AFTER a transform lands at the right plane coordinate ---------

// (64px base: the default brush floods a 32px canvas, masking position — see
// image_rescale.rs. Position-of-paint assertions need the larger canvas.)

#[test]
fn paint_after_flip_lands_at_plane_coords() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);

    e.transform_canvas(OrthoXform::FlipH); // empty canvas; just moves the frame
    paint_feature(&mut e, layer, 40.0, 20.0);

    let px_buf = e.test_readback_canvas();
    let (cx, cy) = red_centroid(&px_buf, w, h);
    assert!(
        (cx - 40.0).abs() < 2.0 && (cy - 20.0).abs() < 2.0,
        "post-flip paint should land at plane ~(40,20), got ({cx},{cy})"
    );
}

#[test]
fn paint_after_rotate_lands_at_plane_coords() {
    // 96×64 → CW → 64×96, origin recentred to (16,-16). Paint at an OFF-CENTRE
    // plane point inside the rotated window and check it lands there (the 64px
    // min dimension keeps the brush from flooding).
    let (w, h) = (96u32, 64u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);

    e.transform_canvas(OrthoXform::Rot90Cw);
    let r = e.canvas_rect();
    assert_eq!((r.width, r.height), (64, 96));

    // Off-centre interior target window-local (40, 60) — ≥20px from every edge
    // so the ~20px brush dab fits and the centroid is clean. Painted via its
    // plane coordinate so this exercises the post-rotate plane→paint mapping.
    let (lx, ly) = (40.0f32, 60.0f32);
    paint_feature(
        &mut e,
        layer,
        r.origin.x as f32 + lx,
        r.origin.y as f32 + ly,
    );

    let buf = e.test_readback_canvas();
    let (cx, cy) = red_centroid(&buf, r.width, r.height);
    assert!(
        (cx - lx).abs() < 2.5 && (cy - ly).abs() < 2.5,
        "post-rotate paint should land at window-local ({lx},{ly}), got ({cx},{cy})"
    );
}

#[test]
fn paint_after_flip_undo_lands_at_plane_coords() {
    let (w, h) = (64u32, 64u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);

    e.transform_canvas(OrthoXform::FlipH);
    e.undo();
    assert_eq!(e.canvas_dimensions(), (w, h));

    paint_feature(&mut e, layer, 40.0, 40.0);
    let buf = e.test_readback_canvas();
    let (cx, cy) = red_centroid(&buf, w, h);
    assert!(
        (cx - 40.0).abs() < 2.0 && (cy - 40.0).abs() < 2.0,
        "paint after undoing a flip should land at plane ~(40,40), got ({cx},{cy})"
    );
}

// ---- A flipped selection masks the mirrored plane pixels -------------------

#[test]
fn paint_through_flipped_selection_masks_mirrored_band() {
    // Select a left-side vertical band, flip the canvas (the selection mirrors
    // to the right), then paint across the whole row: only the now-right band
    // must take paint.
    let (w, h) = (32u32, 32u32);
    let mut e = test_engine(w, h);
    let layer = e.add_raster_layer(None);

    // Band: plane x in [4, 12), full height.
    e.select_rect(4.0, 0.0, 8.0, h as f32, SelectionMode::Replace, false, 0.0);
    e.transform_canvas(OrthoXform::FlipH);
    assert!(e.has_selection(), "flip carries the selection");
    pump(&mut e, 4); // let the selection readback repopulate the cpu cache

    // Mirror of x in [4,12) about a 32-wide canvas → x in [20,28).
    // Paint a row spanning both the (now-unselected) left and (selected) right.
    e.begin_stroke(layer);
    let steps = 64;
    for i in 0..=steps {
        let x = 2.0 + 28.0 * (i as f32 / steps as f32); // sweep x in [2,30]
        e.stroke_to(StrokeOp::BrushStroke {
            x,
            y: 16.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    e.end_stroke();

    let buf = e.test_readback_layer(layer);
    assert!(
        alpha_at(&buf, w, 24, 16) > 0,
        "a pixel inside the mirrored (right) band must be painted"
    );
    assert_eq!(
        alpha_at(&buf, w, 8, 16),
        0,
        "the original (left) band is no longer selected after the flip — must stay clear"
    );
}

// ---- Long undo / redo sequence ---------------------------------------------

#[test]
fn long_transform_sequence_undo_then_redo() {
    let (w, h) = (7u32, 5u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let original = e.test_readback_layer(layer);

    e.transform_canvas(OrthoXform::FlipH);
    e.transform_canvas(OrthoXform::Rot90Cw);
    e.transform_canvas(OrthoXform::FlipV);
    let final_dims = e.canvas_dimensions();
    let final_px = e.test_readback_layer(layer);

    // Undo all three → back to the original document.
    e.undo();
    e.undo();
    e.undo();
    assert_eq!(e.canvas_dimensions(), (w, h));
    assert_eq!(
        e.test_readback_layer(layer),
        original,
        "undo×3 restores the original"
    );

    // Redo all three → back to the final state.
    e.redo();
    e.redo();
    e.redo();
    assert_eq!(e.canvas_dimensions(), final_dims);
    assert_eq!(
        e.test_readback_layer(layer),
        final_px,
        "redo×3 restores the final state"
    );
}

// ---- screen→plane tracks a rotate and its undo -----------------------------

#[test]
fn screen_to_plane_tracks_rotate_and_undo() {
    let (w, h) = (32u32, 24u32);
    let mut e = test_engine(w, h);
    let _layer = e.add_raster_layer(None);
    let (sw, sh) = (200.0f32, 200.0f32);
    e.set_view_transform(0.0, 0.0, 1.0, 0.0, false, sw, sh);

    let (cx0, cy0) = e.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx0 - 16.0).abs() < 1e-2 && (cy0 - 12.0).abs() < 1e-2,
        "pre-rotate centre maps to (16,12), got ({cx0},{cy0})"
    );

    e.transform_canvas(OrthoXform::Rot90Cw); // 24×32, recentred
    let r = e.canvas_rect();
    let (cx1, cy1) = e.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx1 - (r.origin.x as f32 + 12.0)).abs() < 1e-1
            && (cy1 - (r.origin.y as f32 + 16.0)).abs() < 1e-1,
        "post-rotate centre must map to the new window centre, got ({cx1},{cy1})"
    );

    e.undo();
    let (cx2, cy2) = e.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx2 - 16.0).abs() < 1e-2 && (cy2 - 12.0).abs() < 1e-2,
        "after undo, centre maps back to (16,12), got ({cx2},{cy2})"
    );
}
