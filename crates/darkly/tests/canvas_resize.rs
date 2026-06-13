//! Canvas resize & crop-to-selection integration tests.
//!
//! Exercises the canvas-window coordinate model: a non-zero `canvas_origin`
//! (cropped) document, the selection-mask seams (the marquee must keep masking
//! the same *plane* pixels across a crop), and document-only resize undo.
//!
//! Run with: `cargo test -p darkly --test canvas_resize --features testing -- --test-threads=1`

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

fn alpha_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[((y * w + x) * 4 + 3) as usize]
}

/// Paint a horizontal brush stroke at plane-y `py`, sweeping plane-x `[x0, x1)`.
fn paint_row(engine: &mut DarklyEngine, layer_id: LayerId, py: f32, x0: f32, x1: f32) {
    engine.begin_stroke(layer_id);
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

/// MARQUEE regression: a selection made before a crop must keep masking the
/// **same plane pixels** afterward. This exercises the selection-mask
/// re-realization (overlap copy preserving the plane anchor) and the brush
/// composite selection-UV seam (`(p - canvas_origin) / canvas_size`).
#[test]
fn marquee_selection_masks_same_plane_pixels_after_crop() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Select a vertical band: plane x in [8, 32), full height.
    engine.select_rect(8.0, 0.0, 24.0, h as f32, SelectionMode::Replace, false, 0.0);

    // Crop to a window anchored at plane (8, 0), size 40×64 — a NON-ZERO
    // origin. The selection band [8, 32) sits fully inside this window.
    engine.resize_canvas(CanvasRect::from_xywh(8, 0, 40, h));
    assert_eq!(engine.canvas_rect().origin, CanvasPoint::new(8, 0));

    // Paint across the window's plane-x at a plane-y inside the band's height.
    paint_row(&mut engine, layer_id, 32.0, 8.0, 48.0);

    // The raster layer is plane-anchored (origin (0,0), 64×64) and untouched by
    // the crop, so its readback is plane-indexed.
    let px = engine.test_readback_layer(layer_id);

    assert!(
        alpha_at(&px, w, 20, 32) > 0,
        "a selected plane pixel must still be painted through the marquee after crop"
    );
    assert_eq!(
        alpha_at(&px, w, 40, 32),
        0,
        "an unselected plane pixel (inside the new window) must stay transparent"
    );
}

/// Crop moves the canvas window and preserves off-window layer pixels — the
/// raster layer keeps its full plane extent; only display/export is clipped.
#[test]
fn crop_preserves_off_window_layer_pixels() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Paint a full-width row at plane y=10 (before any crop).
    paint_row(&mut engine, layer_id, 10.0, 0.0, w as f32 - 1.0);
    let before = engine.test_readback_layer(layer_id);
    let painted_x = (4..60)
        .find(|&x| alpha_at(&before, w, x, 10) > 0)
        .expect("row should have paint before crop");

    // Crop to a sub-window that EXCLUDES the painted row (window y starts at 20).
    engine.resize_canvas(CanvasRect::from_xywh(0, 20, 40, 30));

    // The painted pixel is now outside the window, but still on the layer.
    let after = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&after, w, painted_x, 10) > 0,
        "off-window layer pixels must be preserved on the layer across a crop"
    );
}

/// Resize/crop is document-only and exactly undoable: undo restores both the
/// origin and the dimensions.
#[test]
fn resize_canvas_undo_restores_window() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    engine.resize_canvas(CanvasRect::from_xywh(12, 8, 30, 40));
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(12, 8, 30, 40));

    engine.undo();
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(0, 0, w, h));

    engine.redo();
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(12, 8, 30, 40));
}

/// PRESENT-PATH regression: the cached view transform embeds the canvas
/// dimensions (`canvas_w/h` as the present shader's sampling-normalization +
/// the canvas center). A resize/crop changes the dims but is otherwise
/// document-only, so the view matrix must be **rebuilt** to match — otherwise
/// the present pass samples the new-size composite through a stale-dim matrix
/// and the image shows stretched/offset until the next pointer event re-pushes
/// the view (the reported "glitch that heals on interaction" / "stretched
/// content" bugs).
///
/// `screen_to_plane` reads the same cached `view_transform` the present pass
/// consumes, so probing it is the present-path invariant without GPU-readback
/// flakiness. With an identity-fit view (pan 0, zoom 1), the screen center
/// resolves to the canvas center `(canvas_w/2, canvas_h/2)` — which tracks the
/// matrix's embedded dims.
#[test]
fn resize_rebuilds_view_transform_for_new_dims() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    let (sw, sh) = (200.0_f32, 200.0_f32);
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, sw, sh);

    // Sanity: screen center maps to the original canvas center (32, 32).
    let (cx0, cy0) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx0 - 32.0).abs() < 1e-2 && (cy0 - 32.0).abs() < 1e-2,
        "pre-resize screen center should map to (32, 32), got ({cx0}, {cy0})"
    );

    // Grow the canvas WITHOUT any intervening set_view_transform.
    engine.resize_canvas(CanvasRect::from_xywh(0, 0, 128, 96));

    let (cx1, cy1) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx1 - 64.0).abs() < 1e-2,
        "screen center must map to the NEW canvas center x (64) after resize, got {cx1}"
    );
    assert!(
        (cy1 - 48.0).abs() < 1e-2,
        "screen center must map to the NEW canvas center y (48) after resize, got {cy1}"
    );

    // Undo reconciles dims back to 64×64 via the same chokepoint — the view
    // matrix must rebuild on undo too (bug #3: undo restores dims but shows
    // stretched).
    engine.undo();
    let (cx2, cy2) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx2 - 32.0).abs() < 1e-2 && (cy2 - 32.0).abs() < 1e-2,
        "after undo, screen center must map back to (32, 32), got ({cx2}, {cy2})"
    );
}

/// FRAME regression: the engine's screen→canvas query must return **plane**
/// coordinates (window-local + `canvas_origin`), not window-local. Tools and the
/// overlay consume this frame; returning window-local after a non-zero-origin
/// crop is exactly the paint-vs-hover-preview offset bug. With an identity-fit
/// view, the screen center resolves to `canvas_origin + window_center`.
#[test]
fn screen_to_plane_includes_canvas_origin() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    let (sw, sh) = (200.0_f32, 200.0_f32);
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, sw, sh);

    // Crop to a NON-ZERO origin window at plane (8, 4), size 40×40.
    engine.resize_canvas(CanvasRect::from_xywh(8, 4, 40, 40));

    // Screen center → window-local center (20, 20) → plane (8+20, 4+20).
    let (px, py) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (px - 28.0).abs() < 1e-2,
        "screen center must map to plane x = origin.x + window_center (28), got {px}"
    );
    assert!(
        (py - 24.0).abs() < 1e-2,
        "screen center must map to plane y = origin.y + window_center (24), got {py}"
    );
}

/// Crop-to-selection sets the canvas window to the selection's plane bounds.
#[test]
fn crop_to_selection_matches_selection_bounds() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    // Selection rect at plane (16, 12), size 20×24.
    engine.select_rect(16.0, 12.0, 20.0, 24.0, SelectionMode::Replace, false, 0.0);
    // Populate the selection's pixel bounds from the CPU cache so the crop has
    // something to read without waiting on the async readback.
    engine.crop_to_selection();

    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(16, 12, 20, 24));
}
