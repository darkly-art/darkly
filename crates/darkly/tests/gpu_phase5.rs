//! Phase 5 GPU integration tests — GPU-authoritative selection mask.
//!
//! These tests construct a real `DarklyEngine` via headless `GpuContext` and
//! exercise the same code paths that users hit.
//!
//! Run with: `cargo test -p darkly --test gpu_phase5`

use darkly::document::SelectionMode;
use darkly::engine::DarklyEngine;
use darkly::engine::types::StrokeOp;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::mask;

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
// Boolean selection modes (Add / Subtract / Intersect)
// ============================================================================

/// Add mode: select left quarter, add right quarter. Middle is unselected.
#[test]
fn selection_add_mode() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_rect(0.0, 0.0, 32.0, h as f32, SelectionMode::Replace, false, 0.0);
    engine.select_rect(96.0, 0.0, 32.0, h as f32, SelectionMode::Add, false, 0.0);

    paint_full_stroke(&mut engine, layer_id, w, h);
    let pixels = engine.test_readback_layer(layer_id);

    assert!(alpha_at(&pixels, w, 16, h / 2) > 0, "left quarter (Replace) should have paint");
    assert_eq!(alpha_at(&pixels, w, 64, h / 2), 0, "middle (not selected) should be transparent");
    assert!(alpha_at(&pixels, w, 112, h / 2) > 0, "right quarter (Add) should have paint");
}

/// Subtract mode: select all, subtract center band.
#[test]
fn selection_subtract_mode() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_all();
    engine.select_rect(48.0, 0.0, 32.0, h as f32, SelectionMode::Subtract, false, 0.0);

    paint_full_stroke(&mut engine, layer_id, w, h);
    let pixels = engine.test_readback_layer(layer_id);

    assert!(alpha_at(&pixels, w, 16, h / 2) > 0, "left (selected) should have paint");
    assert_eq!(alpha_at(&pixels, w, 64, h / 2), 0, "center (subtracted) should be transparent");
    assert!(alpha_at(&pixels, w, 112, h / 2) > 0, "right (selected) should have paint");
}

/// Intersect mode: left half ∩ top half = top-left quadrant only.
#[test]
fn selection_intersect_mode() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_rect(0.0, 0.0, 64.0, h as f32, SelectionMode::Replace, false, 0.0);
    engine.select_rect(0.0, 0.0, w as f32, 64.0, SelectionMode::Intersect, false, 0.0);

    // Paint horizontal strokes at two heights: y=32 (top) and y=96 (bottom).
    engine.begin_stroke(layer_id);
    for x_step in 0..20 {
        let x = x_step as f32 * (w as f32 / 20.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x, y: 32.0, pressure: 1.0,
            x_tilt: 0.0, y_tilt: 0.0, rotation: 0.0, tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 1.0, cg: 0.0, cb: 0.0, ca: 1.0,
        });
    }
    engine.end_stroke();

    engine.begin_stroke(layer_id);
    for x_step in 0..20 {
        let x = x_step as f32 * (w as f32 / 20.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x, y: 96.0, pressure: 1.0,
            x_tilt: 0.0, y_tilt: 0.0, rotation: 0.0, tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 0.0, cg: 1.0, cb: 0.0, ca: 1.0,
        });
    }
    engine.end_stroke();

    let pixels = engine.test_readback_layer(layer_id);

    // Top-left (16, 32) — in intersection.
    assert!(alpha_at(&pixels, w, 16, 32) > 0, "top-left (intersection) should have paint");
    // Top-right (112, 32) — right half, outside intersection.
    assert_eq!(alpha_at(&pixels, w, 112, 32), 0, "top-right should be transparent");
    // Bottom-left (16, 96) — bottom half, outside intersection.
    assert_eq!(alpha_at(&pixels, w, 16, 96), 0, "bottom-left should be transparent");
}

// ============================================================================
// Invert selection
// ============================================================================

#[test]
fn selection_invert() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_rect(0.0, 0.0, 64.0, h as f32, SelectionMode::Replace, false, 0.0);
    engine.invert_selection();

    paint_full_stroke(&mut engine, layer_id, w, h);
    let pixels = engine.test_readback_layer(layer_id);

    assert_eq!(alpha_at(&pixels, w, 16, h / 2), 0, "left (inverted out) should be transparent");
    assert!(alpha_at(&pixels, w, 112, h / 2) > 0, "right (inverted in) should have paint");
}

// ============================================================================
// Select all / clear selection
// ============================================================================

#[test]
fn selection_select_all() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_all();
    assert!(engine.has_selection());

    paint_full_stroke(&mut engine, layer_id, w, h);
    let pixels = engine.test_readback_layer(layer_id);

    assert!(alpha_at(&pixels, w, w / 2, h / 2) > 0, "center should have paint with select_all");
}

#[test]
fn selection_clear() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_rect(0.0, 0.0, 32.0, h as f32, SelectionMode::Replace, false, 0.0);
    assert!(engine.has_selection());
    engine.clear_selection();
    assert!(!engine.has_selection());

    // Paint at right side — should work (no selection masking).
    engine.begin_stroke(layer_id);
    for step in 0..5 {
        engine.stroke_to(StrokeOp::BrushStroke {
            x: 48.0, y: 16.0 + step as f32 * 8.0, pressure: 1.0,
            x_tilt: 0.0, y_tilt: 0.0, rotation: 0.0, tangential_pressure: 0.0,
            time_ms: step as f64 * 16.0,
            cr: 1.0, cg: 0.0, cb: 0.0, ca: 1.0,
        });
    }
    engine.end_stroke();

    let pixels = engine.test_readback_layer(layer_id);
    assert!(alpha_at(&pixels, w, 48, 32) > 0, "right side should have paint after clear_selection");
}

// ============================================================================
// Undo / redo of selection changes
// ============================================================================

#[test]
fn selection_undo_redo() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    assert!(!engine.has_selection());

    // Create a selection on the left half.
    engine.select_rect(0.0, 0.0, 64.0, h as f32, SelectionMode::Replace, false, 0.0);
    assert!(engine.has_selection());

    // Undo the selection — should go back to no selection.
    engine.undo();
    assert!(!engine.has_selection(), "selection should be gone after undo");

    // Redo — selection returns.
    engine.redo();
    assert!(engine.has_selection(), "selection should be back after redo");

    // Paint with the selection active — only left half gets paint.
    paint_full_stroke(&mut engine, layer_id, w, h);
    let px = engine.test_readback_layer(layer_id);
    assert!(alpha_at(&px, w, 16, h / 2) > 0, "left should have paint");
    assert_eq!(alpha_at(&px, w, 112, h / 2), 0, "right should be empty with selection");

    // Undo the stroke, undo the selection, verify right side can be painted.
    engine.undo(); // undo stroke
    engine.undo(); // undo selection
    assert!(!engine.has_selection());

    paint_full_stroke(&mut engine, layer_id, w, h);
    let px = engine.test_readback_layer(layer_id);
    assert!(alpha_at(&px, w, 112, h / 2) > 0, "right should have paint after undo (no masking)");
}

// ============================================================================
// Clear selection contents (delete key)
// ============================================================================

#[test]
fn clear_selection_contents() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    // Paint the full canvas.
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::PaintCircle {
        x: 64.0, y: 64.0, radius: 100.0,
        r: 255, g: 0, b: 0, a: 255,
    });
    engine.end_stroke();

    // Select left half and delete.
    engine.select_rect(0.0, 0.0, 64.0, h as f32, SelectionMode::Replace, false, 0.0);
    engine.clear_selection_contents(layer_id);

    let pixels = engine.test_readback_layer(layer_id);
    assert_eq!(alpha_at(&pixels, w, 16, 64), 0, "left (cleared) should be transparent");
    assert!(alpha_at(&pixels, w, 96, 64) > 0, "right (kept) should still have paint");
}

// ============================================================================
// No selection → painting works normally
// ============================================================================

#[test]
fn no_selection_paints_normally() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    assert!(!engine.has_selection());

    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x: 32.0, y: 32.0, pressure: 1.0,
        x_tilt: 0.0, y_tilt: 0.0, rotation: 0.0, tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 1.0, cg: 0.0, cb: 0.0, ca: 1.0,
    });
    engine.end_stroke();

    let pixels = engine.test_readback_layer(layer_id);
    assert!(alpha_at(&pixels, w, 32, 32) > 0, "center should have paint with no selection");
}

// ============================================================================
// Flat-buffer helpers (rasterize_sdf_r8, contour_segments_r8, pixel_bounds_r8)
// ============================================================================

#[test]
fn rasterize_sdf_r8_rect() {
    let (w, h) = (64, 64);
    let pixels = mask::rasterize_sdf_r8(
        w, h,
        (10, 10, 20, 20),
        |px, py| darkly::sdf::sdf_rect(px, py, 20.0, 20.0, 10.0, 10.0),
        false, 0.0,
    );

    assert_eq!(pixels[(20 * w + 20) as usize], 255, "inside rect should be 255");
    assert_eq!(pixels[(5 * w + 5) as usize], 0, "outside rect should be 0");
}

#[test]
fn pixel_bounds_r8_tight() {
    let (w, h) = (64u32, 64u32);
    let mut data = vec![0u8; (w * h) as usize];

    for y in 30..35 {
        for x in 20..30 {
            data[(y * w + x) as usize] = 255;
        }
    }

    let bounds = mask::pixel_bounds_r8(&data, w, h).unwrap();
    assert_eq!(bounds, [20, 30, 10, 5]);
}

#[test]
fn pixel_bounds_r8_empty() {
    let data = vec![0u8; 64 * 64];
    assert!(mask::pixel_bounds_r8(&data, 64, 64).is_none());
}

#[test]
fn contour_segments_r8_empty() {
    let data = vec![0u8; 64 * 64];
    assert!(mask::contour_segments_r8(&data, 64, 64, 127).is_empty());
}

/// Rectangle contour: every segment should lie on the boundary, and the
/// segments should form a closed loop that traces the rectangle perimeter.
#[test]
fn contour_segments_r8_rectangle_geometry() {
    let (w, h) = (128u32, 128u32);
    let (rx, ry, rw, rh) = (30u32, 20u32, 60u32, 40u32);

    let mut data = vec![0u8; (w * h) as usize];
    for y in ry..ry + rh {
        for x in rx..rx + rw {
            data[(y * w + x) as usize] = 255;
        }
    }

    let segments = mask::contour_segments_r8(&data, w, h, 127);
    assert!(!segments.is_empty(), "rectangle should produce contour segments");

    // Every endpoint must lie on one of the four rectangle edges.
    // Marching squares places the contour at the threshold crossing between
    // inside and outside pixels. With hard-edge (value 0 or 255) data, the
    // crossing is at 0.5px outside the filled block:
    //   left edge:   x = rx - 0.5  (between outside pixel rx-1 and inside rx)
    //   right edge:  x = rx + rw - 0.5  (between inside rx+rw-1 and outside rx+rw)
    //   ... but lerp_edge with binary values gives t=0.5, so the marching
    //   squares endpoint is at the cell corner + 0.5 offset.
    // In practice, for a filled block [rx..rx+rw) x [ry..ry+rh), the contour
    // runs through integer coordinates minus 0.5 on each side. The exact
    // positions depend on the lerp_edge interpolation.
    //
    // Rather than hardcoding the exact offsets, we check that endpoints
    // are within 1px of the expected rect boundary.
    let (left, right) = (rx as f32, (rx + rw) as f32);
    let (top, bottom) = (ry as f32, (ry + rh) as f32);
    let margin = 1.0;

    let on_boundary = |p: [f32; 2]| -> bool {
        let on_left   = (p[0] - left).abs()   < margin && p[1] >= top  - margin && p[1] <= bottom + margin;
        let on_right  = (p[0] - right).abs()  < margin && p[1] >= top  - margin && p[1] <= bottom + margin;
        let on_top    = (p[1] - top).abs()     < margin && p[0] >= left - margin && p[0] <= right  + margin;
        let on_bottom = (p[1] - bottom).abs()  < margin && p[0] >= left - margin && p[0] <= right  + margin;
        on_left || on_right || on_top || on_bottom
    };

    for (i, (a, b)) in segments.iter().enumerate() {
        assert!(on_boundary(*a),
            "segment {i} start ({:.1}, {:.1}) not on rect boundary [{left},{top}]-[{right},{bottom}]",
            a[0], a[1]);
        assert!(on_boundary(*b),
            "segment {i} end ({:.1}, {:.1}) not on rect boundary [{left},{top}]-[{right},{bottom}]",
            b[0], b[1]);
    }

    // The total length of all segments should equal the rectangle perimeter.
    let total_len: f32 = segments.iter()
        .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
        .sum();
    let expected_perimeter = 2.0 * (rw as f32 + rh as f32);
    assert!(
        (total_len - expected_perimeter).abs() < 2.0,
        "total contour length {total_len:.1} should be ~{expected_perimeter:.1} (rect perimeter)"
    );

    // Segments should form a closed loop: for every endpoint, there must
    // be another segment with a matching endpoint (within tolerance).
    let close = |a: [f32; 2], b: [f32; 2]| {
        (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
    };
    for (i, (a, b)) in segments.iter().enumerate() {
        for pt in [a, b] {
            let connects = segments.iter().enumerate().any(|(j, (c, d))| {
                j != i && (close(*pt, *c) || close(*pt, *d))
            });
            assert!(connects,
                "segment {i} endpoint ({:.1}, {:.1}) is dangling (not connected)",
                pt[0], pt[1]);
        }
    }
}

/// Contour of a small circle: endpoints should be within the bounding circle
/// and total length should approximate the circumference.
#[test]
fn contour_segments_r8_circle_geometry() {
    let (w, h) = (128u32, 128u32);
    let (cx, cy, r) = (64.0_f32, 64.0_f32, 20.0_f32);

    let pixels = mask::rasterize_sdf_r8(
        w, h,
        ((cx - r) as i32, (cy - r) as i32, (2.0 * r) as i32, (2.0 * r) as i32),
        |px, py| darkly::sdf::sdf_circle(px, py, cx, cy, r),
        true, 0.0,
    );

    let segments = mask::contour_segments_r8(&pixels, w, h, 127);
    assert!(!segments.is_empty(), "circle should produce contour segments");

    // Every endpoint should be roughly at distance r from the center.
    for (i, (a, b)) in segments.iter().enumerate() {
        for pt in [a, b] {
            let dist = ((pt[0] - cx).powi(2) + (pt[1] - cy).powi(2)).sqrt();
            assert!((dist - r).abs() < 2.0,
                "segment {i} endpoint ({:.1}, {:.1}) is {dist:.1} from center, expected ~{r:.0}",
                pt[0], pt[1]);
        }
    }

    // Total length should approximate circumference = 2πr.
    let total_len: f32 = segments.iter()
        .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
        .sum();
    let expected = 2.0 * std::f32::consts::PI * r;
    assert!(
        (total_len - expected).abs() / expected < 0.05,
        "contour length {total_len:.1} should be ~{expected:.1} (2πr), error > 5%"
    );
}

/// Verify contour_segments_r8 matches AlphaMask::contour_segments for the
/// same rectangular shape.
#[test]
fn contour_segments_r8_matches_tile_version() {
    let (w, h) = (128u32, 128u32);

    let mut flat = vec![0u8; (w * h) as usize];
    for y in 20..60 {
        for x in 30..90 {
            flat[(y * w + x) as usize] = 255;
        }
    }
    let r8_segs = mask::contour_segments_r8(&flat, w, h, 127);

    let tile_mask = darkly::tile::AlphaMask::from_r8(&flat, w, h);
    let tile_segs = tile_mask.contour_segments(0.5);

    assert_eq!(r8_segs.len(), tile_segs.len(),
        "r8 ({}) and tile ({}) segment counts should match",
        r8_segs.len(), tile_segs.len());

    let eps = 0.01;
    let close = |a: [f32; 2], b: [f32; 2]| {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps
    };
    for (i, r8) in r8_segs.iter().enumerate() {
        let found = tile_segs.iter().any(|t| {
            (close(r8.0, t.0) && close(r8.1, t.1))
            || (close(r8.0, t.1) && close(r8.1, t.0))
        });
        assert!(found, "r8 segment {i} ({:?} -> {:?}) not in tile output", r8.0, r8.1);
    }
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
