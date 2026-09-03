//! Image rescale ("Image Size") integration tests.
//!
//! Exercises content-scaling resize: all layer + mask pixels are resampled to
//! new document dimensions (distinct from canvas resize, which only moves the
//! window). Covers the GPU resample (centroid scaling up + down), the lossy
//! per-node undo that restores across a texture extent change (the load-bearing
//! capture→realloc→upload path), the multi-region restore (two layers), mask
//! scaling, and the selection clear.
//!
//! It also carries a **tool-accuracy suite** that mirrors `canvas_resize.rs`:
//! every tool (paint, rect select + marquee mask, marching ants, flood fill,
//! magic wand, transform-from-selection, color pick, view transform) is checked
//! both **after a rescale** and **after undoing the rescale**: the coordinate
//! frames that the rescale-undo bug corrupted. See `image-rescale-undo-handoff.md`.
//!
//! Run with: `cargo test -p darkly --test image_rescale --features testing -- --test-threads=1`

use darkly::coord::{CanvasPoint, CanvasRect};
use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::overlay::{OverlayPrimitive, FLAG_CANVAS_SPACE, KIND_DASHED_LINE};
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Paint a short red horizontal stroke centred near plane `(cx, cy)`.
fn paint_feature(engine: &mut DarklyEngine, layer_id: LayerId, cx: f32, cy: f32) {
    engine.begin_stroke(layer_id).unwrap();
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

/// Upscaling 2x scales a painted feature's position about the origin: a feature
/// at canvas `(cx, cy)` lands near `(2cx, 2cy)` and the canvas dims double.
#[test]
fn feature_centroid_scales_after_2x() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);
    paint_feature(&mut engine, layer, 8.0, 8.0);

    let before = engine.test_readback_canvas();
    let (cx0, cy0) = red_centroid(&before, w, h);

    engine.rescale_image(2 * w, 2 * h);
    assert_eq!(engine.canvas_dimensions(), (2 * w, 2 * h));

    let after = engine.test_readback_canvas();
    let (cx1, cy1) = red_centroid(&after, 2 * w, 2 * h);

    assert!(
        (cx1 - 2.0 * cx0).abs() < 2.0 && (cy1 - 2.0 * cy0).abs() < 2.0,
        "feature centroid should scale ~2x: before ({cx0},{cy0}) after ({cx1},{cy1})"
    );
}

/// Downscaling 0.25x (two box-pyramid halvings) quarters the dims and the
/// feature position, and the content survives the reduction.
#[test]
fn rescale_downscale_025x() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);
    paint_feature(&mut engine, layer, 16.0, 16.0);

    let before = engine.test_readback_canvas();
    let (cx0, cy0) = red_centroid(&before, w, h);

    engine.rescale_image(w / 4, h / 4);
    assert_eq!(engine.canvas_dimensions(), (w / 4, h / 4));

    let after = engine.test_readback_canvas();
    // Content must still be present (the halving pyramid didn't lose it).
    let (cx1, cy1) = red_centroid(&after, w / 4, h / 4);
    assert!(
        (cx1 - 0.25 * cx0).abs() < 2.0 && (cy1 - 0.25 * cy0).abs() < 2.0,
        "feature centroid should scale ~0.25x: before ({cx0},{cy0}) after ({cx1},{cy1})"
    );
}

/// MULTI-REGION regression: a rescale snapshots one GPU region per node. Undo
/// must restore EVERY node's pixels, not just the first: the bug the
/// `gpu_region_entries_mut` loop fixes (the old single-`if let` restored one).
#[test]
fn two_layers_undo_restores_all_pixels() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let l1 = engine.add_raster_layer(None);
    let l2 = engine.add_raster_layer(None);
    paint_feature(&mut engine, l1, 8.0, 8.0);
    paint_feature(&mut engine, l2, 20.0, 20.0);

    let before1 = engine.test_readback_layer(l1);
    let before2 = engine.test_readback_layer(l2);

    engine.rescale_image(2 * w, 2 * h);
    engine.undo();

    let after1 = engine.test_readback_layer(l1);
    let after2 = engine.test_readback_layer(l2);

    assert_eq!(
        after1, before1,
        "layer 1 pixels must be restored exactly on undo"
    );
    assert_eq!(
        after2, before2,
        "layer 2 pixels must be restored exactly on undo"
    );
}

/// EXTENT-CHANGE regression: the undo restore captures the current (opposite-
/// direction) pixels, reallocs the node texture to the entry's extent, then
/// uploads, so undo AND redo both round-trip across the size change.
#[test]
fn rescale_undo_then_redo_pixels() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);
    paint_feature(&mut engine, layer, 10.0, 10.0);

    let before = engine.test_readback_layer(layer);
    engine.rescale_image(2 * w, 2 * h);
    let scaled = engine.test_readback_layer(layer);
    assert_eq!(
        scaled.len(),
        (2 * w * 2 * h * 4) as usize,
        "layer extent should double"
    );

    engine.undo();
    let undone = engine.test_readback_layer(layer);
    assert_eq!(engine.canvas_dimensions(), (w, h));
    assert_eq!(
        undone, before,
        "undo must restore the original pixels exactly"
    );

    engine.redo();
    let redone = engine.test_readback_layer(layer);
    assert_eq!(engine.canvas_dimensions(), (2 * w, 2 * h));
    assert_eq!(
        redone, scaled,
        "redo must restore the resampled pixels exactly"
    );
}

/// A layer mask is a pixel-bearing filter: it must scale in lockstep with
/// its host layer.
#[test]
fn mask_scales_with_layer() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);
    engine.add_mask(layer);
    let mask = engine.test_mask_id(layer).expect("mask filter present");

    assert_eq!(
        engine.test_node_pixel_bounds(mask),
        Some(CanvasRect::from_xywh(0, 0, w, h))
    );

    engine.rescale_image(2 * w, 2 * h);

    assert_eq!(
        engine.test_node_pixel_bounds(mask),
        Some(CanvasRect::from_xywh(0, 0, 2 * w, 2 * h)),
        "mask extent must scale with its host layer"
    );
    // GPU texture (R8, 1 byte/px) was actually reallocated to the new extent.
    assert_eq!(
        engine.test_readback_layer(mask).len(),
        (2 * w * 2 * h) as usize
    );
}

/// Rescale clears the active selection (folded into the single undo step);
/// undo restores it.
#[test]
fn rescale_clears_active_selection() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);
    engine.select_rect(8.0, 8.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    assert!(
        engine.has_selection(),
        "selection should be active before rescale"
    );

    engine.rescale_image(2 * w, 2 * h);
    assert!(
        !engine.has_selection(),
        "rescale should clear the active selection"
    );

    engine.undo();
    assert!(engine.has_selection(), "undo should restore the selection");
}

/// A no-op rescale (unchanged dims) pushes no undo step.
#[test]
fn rescale_noop_when_unchanged() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    assert!(!engine.test_can_undo());
    engine.rescale_image(w, h);
    assert!(
        !engine.test_can_undo(),
        "a same-dims rescale must not push an undo step"
    );
}

// =========================================================================
// Tool-accuracy suite: every tool, after a rescale AND after undoing it.
//
// These guard the coordinate frames that the rescale-undo bug corrupted:
// after `rescale_image` (and after `undo()` back to the original dims), a
// tool driven at a known plane coordinate must land exactly there. The
// engine math is the source of truth here; the frontend's borrow-free
// `viewMatrices` mirror is exercised by the vitest suite (the engine test
// at the default origin can pass while the app is broken; see
// docs/coordinate-systems.md).
// =========================================================================

/// Alpha at plane `(x, y)` of a plane-anchored (origin-0) layer readback.
fn alpha_at(px: &[u8], w: u32, x: u32, y: u32) -> u8 {
    px[((y * w + x) * 4 + 3) as usize]
}

/// Paint a horizontal red brush row at plane-y `py`, sweeping plane-x `[x0, x1]`.
fn paint_row(engine: &mut DarklyEngine, layer_id: LayerId, py: f32, x0: f32, x1: f32) {
    engine.begin_stroke(layer_id).unwrap();
    let steps = 48;
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * (i as f32 / steps as f32);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: py,
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

/// Bounding box `(min_x, min_y, max_x, max_y)` of all marching-ants
/// (`KIND_DASHED_LINE`, `FLAG_CANVAS_SPACE`) vertices, in plane space.
fn ants_bbox(prims: &[OverlayPrimitive]) -> (f32, f32, f32, f32) {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut found = false;
    for p in prims {
        if p.kind != KIND_DASHED_LINE || (p.flags & FLAG_CANVAS_SPACE) == 0 {
            continue;
        }
        for &[x, y] in &[p.p0, p.p1] {
            found = true;
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
    }
    assert!(found, "no marching-ants primitives present");
    (minx, miny, maxx, maxy)
}

/// Drive pending async GPU readbacks to completion (flush + render), `n` frames.
fn pump(engine: &mut DarklyEngine, n: u32) {
    for _ in 0..n {
        engine.test_flush_readbacks();
        engine.render(0.0);
    }
}

/// A full-canvas RGBA buffer (transparent) with a solid opaque red square at
/// plane `[x0, x1) × [y0, y1)`.
fn rgba_with_red_square(w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = 255;
            rgba[i + 3] = 255;
        }
    }
    rgba
}

/// Flood fill `layer` at plane `(x, y)` with `color`.
fn flood_fill(
    engine: &mut DarklyEngine,
    layer_id: LayerId,
    x: f32,
    y: f32,
    color: [u8; 4],
    tol: u8,
) {
    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x,
        y,
        r: color[0],
        g: color[1],
        b: color[2],
        a: color[3],
        tolerance: tol,
    });
    engine.end_stroke();
}

// ---- Painting --------------------------------------------------------------

/// Paint lands at the intended plane coordinate AFTER a rescale (the resampled
/// canvas's paint frame is correct).
#[test]
fn paint_lands_at_plane_coords_after_rescale() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h); // 64×64, empty layer rescaled
    paint_feature(&mut engine, layer, 40.0, 40.0);

    let px = engine.test_readback_layer(layer);
    let (cx, cy) = red_centroid(&px, 2 * w, 2 * h);
    assert!(
        (cx - 40.0).abs() < 2.0 && (cy - 40.0).abs() < 2.0,
        "post-rescale paint landed at ({cx}, {cy}), expected ~(40, 40)"
    );
}

/// Paint lands at the intended plane coordinate AFTER undoing a rescale: the
/// engine-level analogue of the reported "coords don't match the tool" bug.
/// Uses a 64px base (the default brush floods a 32px canvas, masking position).
#[test]
fn paint_lands_at_plane_coords_after_rescale_undo() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h);
    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (w, h));

    paint_feature(&mut engine, layer, 40.0, 40.0);
    let px = engine.test_readback_layer(layer);
    let (cx, cy) = red_centroid(&px, w, h);
    assert!(
        (cx - 40.0).abs() < 2.0 && (cy - 40.0).abs() < 2.0,
        "post-undo paint landed at ({cx}, {cy}), expected ~(40, 40)"
    );
}

// ---- Rect selection: the marquee masks the right plane pixels --------------

/// A selection made AFTER a rescale masks paint to the selected plane pixels.
#[test]
fn marquee_masks_same_plane_pixels_after_rescale() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h); // 128×128
                                        // Select a vertical band: plane x in [16, 64), full height.
    engine.select_rect(
        16.0,
        0.0,
        48.0,
        (2 * h) as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    // Paint a row spanning selected (x<64) and unselected (x>=64) plane-x.
    paint_row(&mut engine, layer, 40.0, 16.0, 96.0);

    let px = engine.test_readback_layer(layer);
    assert!(
        alpha_at(&px, 2 * w, 40, 40) > 0,
        "a selected plane pixel must be painted through the marquee after rescale"
    );
    assert_eq!(
        alpha_at(&px, 2 * w, 80, 40),
        0,
        "an unselected plane pixel must stay transparent after rescale"
    );
}

/// A selection made AFTER undoing a rescale masks paint to the selected plane
/// pixels: the reported bug, at engine level.
#[test]
fn marquee_masks_same_plane_pixels_after_rescale_undo() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h);
    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (w, h));

    // Plane band x in [8, 32), full height.
    engine.select_rect(8.0, 0.0, 24.0, h as f32, SelectionMode::Replace, false, 0.0);
    paint_row(&mut engine, layer, 32.0, 8.0, 56.0);

    let px = engine.test_readback_layer(layer);
    assert!(
        alpha_at(&px, w, 20, 32) > 0,
        "selected plane pixel must be painted through the marquee after undo"
    );
    assert_eq!(
        alpha_at(&px, w, 44, 32),
        0,
        "unselected plane pixel must stay transparent after undo"
    );
}

// ---- Marching ants track the selection's plane bounds ----------------------

#[test]
fn marching_ants_track_selection_after_rescale() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h); // 64×64
    engine.select_rect(20.0, 16.0, 12.0, 10.0, SelectionMode::Replace, false, 0.0);

    let (minx, miny, maxx, maxy) = ants_bbox(&engine.test_selection_overlay());
    assert!(
        (minx - 20.0).abs() <= 2.0 && (miny - 16.0).abs() <= 2.0,
        "ants min ({minx}, {miny}) must track plane origin (20, 16) after rescale"
    );
    assert!(
        (maxx - 32.0).abs() <= 2.0 && (maxy - 26.0).abs() <= 2.0,
        "ants max ({maxx}, {maxy}) must track plane far edge (32, 26) after rescale"
    );
}

#[test]
fn marching_ants_track_selection_after_rescale_undo() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h);
    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (w, h));

    engine.select_rect(16.0, 12.0, 10.0, 8.0, SelectionMode::Replace, false, 0.0);
    let (minx, miny, maxx, maxy) = ants_bbox(&engine.test_selection_overlay());
    assert!(
        (minx - 16.0).abs() <= 2.0 && (miny - 12.0).abs() <= 2.0,
        "ants min ({minx}, {miny}) must track plane origin (16, 12) after undo"
    );
    assert!(
        (maxx - 26.0).abs() <= 2.0 && (maxy - 20.0).abs() <= 2.0,
        "ants max ({maxx}, {maxy}) must track plane far edge (26, 20) after undo"
    );
}

// ---- Flood fill ------------------------------------------------------------

/// Flood fill's seed maps to the right plane pixel AFTER a rescale: a fill
/// seeded inside the (scaled) red square recolors the square, not the bg.
#[test]
fn flood_fill_fills_plane_region_after_rescale() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    // Red square at plane [8, 16) on a 32² layer.
    let rgba = rgba_with_red_square(w, h, 8, 8, 16, 16);
    let layer = engine.paste_image(w, h, &rgba, 0, 0, None);

    engine.rescale_image(2 * w, 2 * h); // square scales to plane [16, 32)
                                        // Seed inside the scaled square; recolor red → green.
    flood_fill(&mut engine, layer, 24.0, 24.0, [0, 255, 0, 255], 50);
    pump(&mut engine, 8);

    let px = engine.test_readback_layer(layer);
    let i = ((24 * 2 * w + 24) * 4) as usize;
    assert!(
        px[i] < 80 && px[i + 1] > 150,
        "fill seed inside the scaled square must recolor it green, got {:?}",
        &px[i..i + 4]
    );
    // A background pixel (outside the square) must be untouched (transparent).
    assert_eq!(
        alpha_at(&px, 2 * w, 4, 4),
        0,
        "flood fill must not leak outside the seeded region after rescale"
    );
}

/// Flood fill's seed maps to the right plane pixel AFTER undoing a rescale.
#[test]
fn flood_fill_fills_plane_region_after_rescale_undo() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let rgba = rgba_with_red_square(w, h, 8, 8, 16, 16);
    let layer = engine.paste_image(w, h, &rgba, 0, 0, None);

    engine.rescale_image(2 * w, 2 * h);
    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (w, h));

    // Square is back at plane [8, 16); seed inside it.
    flood_fill(&mut engine, layer, 12.0, 12.0, [0, 255, 0, 255], 50);
    pump(&mut engine, 8);

    let px = engine.test_readback_layer(layer);
    let i = ((12 * w + 12) * 4) as usize;
    assert!(
        px[i] < 80 && px[i + 1] > 150,
        "post-undo fill seed inside the square must recolor it green, got {:?}",
        &px[i..i + 4]
    );
    assert_eq!(
        alpha_at(&px, w, 2, 2),
        0,
        "flood fill must not leak outside the seeded region after undo"
    );
}

// ---- Magic wand ------------------------------------------------------------

/// Magic wand seeded inside the (scaled) red square selects the square's plane
/// bounds AFTER a rescale.
#[test]
fn magic_wand_selects_plane_region_after_rescale() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let rgba = rgba_with_red_square(w, h, 8, 8, 16, 16);
    let layer = engine.paste_image(w, h, &rgba, 0, 0, None);

    engine.rescale_image(2 * w, 2 * h); // square → plane [16, 32)
    engine.select_magic_wand(layer, CanvasPoint::new(24, 24), 50, SelectionMode::Replace);
    pump(&mut engine, 8);

    assert!(
        engine.has_selection(),
        "magic wand should produce a selection"
    );
    let (minx, miny, maxx, maxy) = ants_bbox(&engine.test_selection_overlay());
    assert!(
        (minx - 16.0).abs() <= 3.0 && (miny - 16.0).abs() <= 3.0,
        "wand selection min ({minx}, {miny}) must track scaled square origin (16, 16)"
    );
    assert!(
        (maxx - 32.0).abs() <= 3.0 && (maxy - 32.0).abs() <= 3.0,
        "wand selection max ({maxx}, {maxy}) must track scaled square far edge (32, 32)"
    );
}

/// Magic wand selects the square's plane bounds AFTER undoing a rescale.
#[test]
fn magic_wand_selects_plane_region_after_rescale_undo() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let rgba = rgba_with_red_square(w, h, 8, 8, 16, 16);
    let layer = engine.paste_image(w, h, &rgba, 0, 0, None);

    engine.rescale_image(2 * w, 2 * h);
    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (w, h));

    engine.select_magic_wand(layer, CanvasPoint::new(12, 12), 50, SelectionMode::Replace);
    pump(&mut engine, 8);

    assert!(
        engine.has_selection(),
        "magic wand should produce a selection"
    );
    let (minx, miny, maxx, maxy) = ants_bbox(&engine.test_selection_overlay());
    assert!(
        (minx - 8.0).abs() <= 3.0 && (miny - 8.0).abs() <= 3.0,
        "wand selection min ({minx}, {miny}) must track square origin (8, 8) after undo"
    );
    assert!(
        (maxx - 16.0).abs() <= 3.0 && (maxy - 16.0).abs() <= 3.0,
        "wand selection max ({maxx}, {maxy}) must track square far edge (16, 16) after undo"
    );
}

// ---- Transform from selection ---------------------------------------------

/// `begin_transform` from a selection hands a PLANE source origin AFTER a
/// rescale (not a window-local one).
#[test]
fn transform_from_selection_plane_origin_after_rescale() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h); // 64×64
    paint_feature(&mut engine, layer, 32.0, 28.0);
    engine.select_rect(24.0, 20.0, 16.0, 12.0, SelectionMode::Replace, false, 0.0);

    assert!(engine.begin_transform(layer), "transform should set up");
    let (sox, soy, sw, sh, _m) = engine.floating_info().expect("floating active");
    assert!(
        (sox - 24.0).abs() <= 1.0 && (soy - 20.0).abs() <= 1.0,
        "source origin ({sox}, {soy}) must be the selection's plane origin (24, 20)"
    );
    assert!(
        (sw - 16.0).abs() <= 1.0 && (sh - 12.0).abs() <= 1.0,
        "source size ({sw}, {sh}) should match the selection (16, 12)"
    );
}

/// `begin_transform` from a selection hands a PLANE source origin AFTER undoing
/// a rescale.
#[test]
fn transform_from_selection_plane_origin_after_rescale_undo() {
    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    engine.rescale_image(2 * w, 2 * h);
    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (w, h));

    paint_feature(&mut engine, layer, 16.0, 14.0);
    engine.select_rect(12.0, 10.0, 8.0, 6.0, SelectionMode::Replace, false, 0.0);

    assert!(engine.begin_transform(layer), "transform should set up");
    let (sox, soy, sw, sh, _m) = engine.floating_info().expect("floating active");
    assert!(
        (sox - 12.0).abs() <= 1.0 && (soy - 10.0).abs() <= 1.0,
        "source origin ({sox}, {soy}) must be the selection's plane origin (12, 10) after undo"
    );
    assert!(
        (sw - 8.0).abs() <= 1.0 && (sh - 6.0).abs() <= 1.0,
        "source size ({sw}, {sh}) should match the selection (8, 6) after undo"
    );
}

// ---- Color pick ------------------------------------------------------------

/// A merged color pick reads the plane pixel AFTER a rescale.
#[test]
fn color_pick_reads_plane_pixel_after_rescale() {
    use darkly::engine::PickSource;

    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    engine.rescale_image(2 * w, 2 * h); // 64×64

    // Paste a 64×64 layer: red everywhere, green 4×4 at plane (40, 40).
    let mut rgba = vec![0u8; (2 * w * 2 * h * 4) as usize];
    for px in rgba.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[200, 0, 0, 255]);
    }
    for y in 40..44 {
        for x in 40..44 {
            let i = ((y * 2 * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[0, 200, 0, 255]);
        }
    }
    let _layer = engine.paste_image(2 * w, 2 * h, &rgba, 0, 0, None);
    let _ = engine.test_readback_canvas(); // force the merged composite

    engine.pick_color(41.0, 41.0, PickSource::Merged);
    engine.test_flush_readbacks();
    let c = engine.last_picked_color();
    assert!(
        c[1] > 150 && c[0] < 80,
        "merged pick after rescale must read the plane pixel (green); got {c:?}"
    );
}

/// A merged color pick reads the plane pixel AFTER undoing a rescale.
#[test]
fn color_pick_reads_plane_pixel_after_rescale_undo() {
    use darkly::engine::PickSource;

    let (w, h) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    engine.rescale_image(2 * w, 2 * h);
    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (w, h));

    // Paste a 32×32 layer: red everywhere, green 4×4 at plane (20, 20).
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for px in rgba.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[200, 0, 0, 255]);
    }
    for y in 20..24 {
        for x in 20..24 {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[0, 200, 0, 255]);
        }
    }
    let _layer = engine.paste_image(w, h, &rgba, 0, 0, None);
    let _ = engine.test_readback_canvas();

    engine.pick_color(21.0, 21.0, PickSource::Merged);
    engine.test_flush_readbacks();
    let c = engine.last_picked_color();
    assert!(
        c[1] > 150 && c[0] < 80,
        "merged pick after undo must read the plane pixel (green); got {c:?}"
    );
}

// ---- View transform / screen→plane ----------------------------------------

/// The cached view transform (which `screen_to_plane` consumes, same as the
/// present pass) tracks the new dims after a rescale and reverts on undo.
#[test]
fn screen_to_plane_tracks_dims_through_rescale_and_undo() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    let (sw, sh) = (200.0_f32, 200.0_f32);
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, sw, sh);

    let (cx0, cy0) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx0 - 32.0).abs() < 1e-2 && (cy0 - 32.0).abs() < 1e-2,
        "pre-rescale center should map to (32, 32), got ({cx0}, {cy0})"
    );

    engine.rescale_image(2 * w, 2 * h); // 128×128, center → (64, 64)
    let (cx1, cy1) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx1 - 64.0).abs() < 1e-2 && (cy1 - 64.0).abs() < 1e-2,
        "post-rescale center must map to the new canvas center (64, 64), got ({cx1}, {cy1})"
    );

    engine.undo();
    let (cx2, cy2) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx2 - 32.0).abs() < 1e-2 && (cy2 - 32.0).abs() < 1e-2,
        "after undo, center must map back to (32, 32), got ({cx2}, {cy2})"
    );
}
