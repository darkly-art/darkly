//! Phase 5 GPU integration tests — engine-level selection masking and transform bounds.
//!
//! These tests construct a real `DarklyEngine` via headless `GpuContext` and
//! exercise the same code paths that users hit. They are written to **fail**
//! before the GPU selection mask migration and pass after.
//!
//! Run with: `cargo test -p darkly --test gpu_phase5`

use darkly::document::SelectionMode;
use darkly::engine::DarklyEngine;
use darkly::engine::types::StrokeOp;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

/// Create a headless DarklyEngine with the given canvas dimensions.
fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

// ============================================================================
// 0b. Brush stroke respects selection
// ============================================================================

/// Select the left half of the canvas, paint a brush stroke through the center.
/// The left half (selected) should have paint; the right half should not.
///
/// This test MUST FAIL before Phase 1d fixes `brush_stroke_to()` to use the
/// real selection bind group instead of `default_selection_bind_group`.
#[test]
fn engine_brush_stroke_respects_selection() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    // Select left half of canvas.
    engine.select_rect(0.0, 0.0, (w / 2) as f32, h as f32, SelectionMode::Replace, false, 0.0);

    // Paint a brush stroke horizontally through the center of the canvas.
    engine.begin_stroke(layer_id);
    for x_step in 0..20 {
        let x = x_step as f32 * (w as f32 / 20.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: (h / 2) as f32,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 1.0, cg: 0.0, cb: 0.0, ca: 1.0,
        });
    }
    engine.end_stroke();

    // Readback the layer texture.
    let pixels = engine.test_readback_layer(layer_id);

    // Sample a point on the left side (selected), within the stroke path.
    // (32, 64) — left quarter, vertical center.
    let left_idx = ((h / 2) * w + w / 4) as usize * 4;
    let left_alpha = pixels[left_idx + 3];

    // Sample a point on the right side (unselected), within the stroke path.
    // (96, 64) — right quarter, vertical center.
    let right_idx = ((h / 2) * w + 3 * w / 4) as usize * 4;
    let right_alpha = pixels[right_idx + 3];

    assert!(
        left_alpha > 0,
        "left side (selected) should have paint, alpha = {left_alpha}"
    );
    assert_eq!(
        right_alpha, 0,
        "right side (unselected) should be transparent, alpha = {right_alpha}"
    );
}

// ============================================================================
// 0c. Transform bounds are tight (pixel-level, not tile-aligned)
// ============================================================================

/// Create a non-tile-aligned selection, paint inside it, then begin_transform.
/// The floating content dimensions should match the selection's pixel bounds,
/// not tile-aligned bounds.
///
/// This test MUST FAIL before Phase 2a replaces `sel.bounding_rect()` with
/// `GpuSelection::pixel_bounds()` in `begin_transform()`.
#[test]
fn engine_transform_bounds_are_tight() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    // Non-tile-aligned selection: 30×45 at offset (17, 23).
    let sel_x = 17.0_f32;
    let sel_y = 23.0_f32;
    let sel_w = 30.0_f32;
    let sel_h = 45.0_f32;

    engine.select_rect(sel_x, sel_y, sel_w, sel_h, SelectionMode::Replace, false, 0.0);

    // Paint something inside the selection so there's content to transform.
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::PaintCircle {
        x: sel_x + sel_w / 2.0,
        y: sel_y + sel_h / 2.0,
        radius: 10.0,
        r: 255, g: 0, b: 0, a: 255,
    });
    engine.end_stroke();

    // Begin transform — this should use the selection bounds.
    let started = engine.begin_transform(layer_id);
    assert!(started, "begin_transform should succeed synchronously with a selection");

    let (origin_x, origin_y, float_w, float_h, _matrix) = engine
        .floating_info()
        .expect("floating content should exist after begin_transform");

    // The floating content should have tight pixel bounds matching the
    // selection rect, NOT tile-aligned bounds (which would be multiples of 64).
    // Allow ±1 pixel tolerance for rounding.
    let expected_w = sel_w as u32;
    let expected_h = sel_h as u32;

    assert!(
        (float_w as i32 - expected_w as i32).unsigned_abs() <= 1,
        "floating width should be ~{expected_w} (selection width), got {float_w}"
    );
    assert!(
        (float_h as i32 - expected_h as i32).unsigned_abs() <= 1,
        "floating height should be ~{expected_h} (selection height), got {float_h}"
    );
    assert!(
        (origin_x as i32 - sel_x as i32).abs() <= 1,
        "floating origin X should be ~{sel_x}, got {origin_x}"
    );
    assert!(
        (origin_y as i32 - sel_y as i32).abs() <= 1,
        "floating origin Y should be ~{sel_y}, got {origin_y}"
    );
}
