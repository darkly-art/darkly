//! Engine-level GPU integration tests: brush stroke + selection, transform bounds,
//! cut/paste precision, lasso performance.
//!
//! These tests construct a real `DarklyEngine` via headless `GpuContext` and
//! exercise the same code paths that users hit.
//! Run with: `cargo test -p darkly --test engine`

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

/// Paint a horizontal brush stroke across the canvas at vertical center.
fn paint_full_stroke(engine: &mut DarklyEngine, layer_id: u64, w: u32, h: u32) {
    engine.begin_stroke(layer_id);
    for x_step in 0..20 {
        let x = x_step as f32 * (w as f32 / 20.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: (h / 2) as f32,
            pressure: 1.0,
            x_tilt: 0.0, y_tilt: 0.0,
            rotation: 0.0, tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 1.0, cg: 0.0, cb: 0.0, ca: 1.0,
        });
    }
    engine.end_stroke();
}

/// Sample the alpha channel at (x, y) from an RGBA pixel buffer.
fn alpha_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[((y * w + x) * 4 + 3) as usize]
}

// ============================================================================
// Brush stroke respects selection
// ============================================================================

#[test]
fn engine_brush_stroke_respects_selection() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_rect(0.0, 0.0, (w / 2) as f32, h as f32, SelectionMode::Replace, false, 0.0);
    paint_full_stroke(&mut engine, layer_id, w, h);

    let pixels = engine.test_readback_layer(layer_id);
    assert!(alpha_at(&pixels, w, w / 4, h / 2) > 0, "left (selected) should have paint");
    assert_eq!(alpha_at(&pixels, w, 3 * w / 4, h / 2), 0, "right (unselected) should be transparent");
}

// ============================================================================
// Transform bounds are tight (pixel-level, not tile-aligned)
// ============================================================================

#[test]
fn engine_transform_bounds_are_tight() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    let sel_x = 17.0_f32;
    let sel_y = 23.0_f32;
    let sel_w = 30.0_f32;
    let sel_h = 45.0_f32;

    engine.select_rect(sel_x, sel_y, sel_w, sel_h, SelectionMode::Replace, false, 0.0);

    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::PaintCircle {
        x: sel_x + sel_w / 2.0,
        y: sel_y + sel_h / 2.0,
        radius: 10.0,
        r: 255, g: 0, b: 0, a: 255,
    });
    engine.end_stroke();

    let started = engine.begin_transform(layer_id);
    assert!(started, "begin_transform should succeed with a selection");

    let (origin_x, origin_y, float_w, float_h, _) = engine.floating_info().unwrap();

    assert!((float_w as i32 - sel_w as i32).unsigned_abs() <= 1,
        "width should be ~{}, got {float_w}", sel_w as u32);
    assert!((float_h as i32 - sel_h as i32).unsigned_abs() <= 1,
        "height should be ~{}, got {float_h}", sel_h as u32);
    assert!((origin_x as i32 - sel_x as i32).abs() <= 1,
        "origin X should be ~{sel_x}, got {origin_x}");
    assert!((origin_y as i32 - sel_y as i32).abs() <= 1,
        "origin Y should be ~{sel_y}, got {origin_y}");
}

// ============================================================================
// Cut+paste leaves no border (regression test for uint8_mult fix)
// ============================================================================

/// Paint a solid region, make an antialiased selection, cut, paste to a new
/// layer, then verify that `remaining + pasted == original` per channel.
///
/// The bug: the old code erased via GPU float blend and copied via CPU integer
/// math, producing a rounding mismatch at antialiased selection edges. This
/// left a thin border of residual pixels on the source layer.
#[test]
fn cut_paste_no_border() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    // Paint varied-channel content so rounding errors show in every channel.
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::PaintCircle {
        x: 64.0, y: 64.0, radius: 100.0,
        r: 200, g: 100, b: 50, a: 230,
    });
    engine.end_stroke();

    let original = engine.test_readback_layer(layer_id);

    // Antialiased selection — the AA edge is where the mismatch appears.
    engine.select_rect(20.0, 20.0, 60.0, 60.0, SelectionMode::Replace, true, 0.0);

    // Cut.
    engine.cut(layer_id);

    // Block until the async GPU readback completes and the cut is fully applied.
    engine.test_flush_readbacks();
    let clip = engine.poll_copy_result().expect("cut should produce a clipboard result");

    let remaining = engine.test_readback_layer(layer_id);

    // Paste onto a new layer at the same position.
    let paste_id = engine.paste_image(
        clip.width, clip.height, &clip.rgba,
        clip.offset_x, clip.offset_y, Some(layer_id),
    );
    let pasted = engine.test_readback_layer(paste_id);

    // For every originally-painted pixel, verify:
    // remaining[ch] + pasted[ch] == original[ch]   (exact, no ±1 tolerance)
    let mut mismatches = 0u32;
    let mut worst_error = 0i32;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize * 4;
            if original[i + 3] == 0 { continue; }

            for ch in 0..4 {
                let o = original[i + ch] as i32;
                let r = remaining[i + ch] as i32;
                let p = pasted[i + ch] as i32;
                let error = (r + p - o).abs();
                if error > 0 {
                    mismatches += 1;
                    worst_error = worst_error.max(error);
                }
            }
        }
    }

    // With correct integer math (uint8_mult), there should be zero mismatches.
    // The old float-based erase produced errors of 1-3 per channel at every
    // AA-edge pixel, adding up to hundreds of mismatches.
    assert!(mismatches == 0,
        "{mismatches} channel mismatches (worst error: {worst_error}). \
         remaining + pasted should exactly reconstruct original.");
}

// ============================================================================
// Lasso selection performance (regression test for scanline fill)
// ============================================================================

/// Lasso-select a 200-vertex polygon through the engine and verify it completes
/// in bounded time. The old SDF path was O(pixels × edges) — 489ms for 182 verts
/// on WASM. The scanline path is O(pixels + edges × height).
///
/// Also verifies correctness: painting inside the lasso works, painting outside
/// is masked.
#[test]
fn lasso_selection_performance_and_correctness() {
    let (w, h) = (1024, 1024);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    // Generate a circle polygon with 200 vertices — similar to a real lasso.
    let cx = 500.0_f32;
    let cy = 500.0_f32;
    let r = 200.0_f32;
    let n_verts = 200;
    let vertices: Vec<[f32; 2]> = (0..n_verts)
        .map(|i| {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / n_verts as f32;
            [cx + r * angle.cos(), cy + r * angle.sin()]
        })
        .collect();

    // Time the full select_lasso call.
    let start = std::time::Instant::now();
    engine.select_lasso(&vertices, SelectionMode::Replace, true, 0.0);
    let elapsed = start.elapsed();

    let ms = elapsed.as_secs_f64() * 1000.0;
    eprintln!("select_lasso({n_verts} verts, {w}x{h}): {ms:.1}ms");

    // Must complete in <50ms on native. The old SDF path took ~200ms+ here.
    assert!(ms < 50.0,
        "select_lasso with {n_verts} verts took {ms:.1}ms, expected <50ms");

    assert!(engine.has_selection());

    // Correctness: paint across canvas, verify masking works.
    engine.begin_stroke(layer_id);
    for x_step in 0..40 {
        let x = x_step as f32 * (w as f32 / 40.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: cy,
            pressure: 1.0,
            x_tilt: 0.0, y_tilt: 0.0,
            rotation: 0.0, tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 1.0, cg: 0.0, cb: 0.0, ca: 1.0,
        });
    }
    engine.end_stroke();

    let pixels = engine.test_readback_layer(layer_id);

    // Center of polygon (500, 500) — should have paint.
    assert!(alpha_at(&pixels, w, cx as u32, cy as u32) > 0,
        "center of lasso should have paint");

    // Well outside polygon (50, 500) — 450px left of center, outside r=200.
    assert_eq!(alpha_at(&pixels, w, 50, cy as u32), 0,
        "outside lasso should be transparent");
}
