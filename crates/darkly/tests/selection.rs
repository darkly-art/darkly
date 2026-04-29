//! Selection GPU integration tests: boolean modes, invert, select all/clear,
//! undo/redo, clear contents, mask helpers, contour extraction.
//!
//! Combines low-level GpuPaintTarget selection tests and engine-level selection tests.
//! Run with: `cargo test -p darkly --test selection`

use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::paint_target::{GpuPaintTarget, PaintPipelines};
use darkly::gpu::test_utils::*;
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
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
}

/// Sample the alpha channel at (x, y) from an RGBA pixel buffer.
fn alpha_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[((y * w + x) * 4 + 3) as usize]
}

fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test"),
    })
}

fn submit(queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) {
    queue.submit([encoder.finish()]);
}

fn pixel_at(pixels: &[u8], w: u32, x: u32, y: u32, bpp: u32) -> &[u8] {
    let offset = ((y * w + x) * bpp) as usize;
    &pixels[offset..offset + bpp as usize]
}

// ============================================================================
// Low-level selection tests (GpuPaintTarget API)
// ============================================================================

/// Gradient with selection mask: only the masked area should receive the gradient.
#[test]
fn gpu_gradient_with_selection() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);

    // Create selection mask: left half = 255, right half = 0.
    let mut sel_data = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w / 2 {
            sel_data[(y * w + x) as usize] = 255;
        }
    }
    let (sel_tex, _) = create_test_texture_with_format(
        &device,
        &queue,
        w,
        h,
        &sel_data,
        wgpu::TextureFormat::R8Unorm,
    );
    let sel_view = sel_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let sel_bg = pipelines.create_selection_bind_group(&device, &sel_view, &sampler);

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.linear_gradient(
        &mut enc,
        &pipelines,
        &queue,
        0.0,
        0.0,
        64.0,
        0.0,
        [255, 0, 0, 255],
        [0, 0, 255, 255],
        Some(&sel_bg),
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Left half should have gradient.
    let left = pixel_at(&pixels, w, 0, 32, 4);
    assert!(left[3] > 0, "left half should have content, A={}", left[3]);

    // Right half should still be transparent (outside selection).
    let right = pixel_at(&pixels, w, 48, 32, 4);
    assert_eq!(
        right[3], 0,
        "right half should be transparent, A={}",
        right[3]
    );
}

/// Fill layer with red, create selection (left half), clear selection → left half transparent.
#[test]
fn gpu_clear_selection_contents() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Fill with red.
    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, view) = create_test_texture(&device, &queue, w, h, &red);
    let pipelines = PaintPipelines::new(&device, &queue);

    // Create selection mask: left half = 255.
    let mut sel_data = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w / 2 {
            sel_data[(y * w + x) as usize] = 255;
        }
    }
    let (sel_tex, _) = create_test_texture_with_format(
        &device,
        &queue,
        w,
        h,
        &sel_data,
        wgpu::TextureFormat::R8Unorm,
    );
    let sel_view = sel_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let sel_bg = pipelines.create_selection_bind_group(&device, &sel_view, &sampler);

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    // Erase within selection.
    let mut enc = encoder(&device);
    target.erase_with_selection(&mut enc, &pipelines, &queue, &sel_bg);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Left half should be erased (alpha = 0).
    let left = pixel_at(&pixels, w, 10, 32, 4);
    assert_eq!(left[3], 0, "left half should be erased, A={}", left[3]);

    // Right half should still be red.
    let right = pixel_at(&pixels, w, 50, 32, 4);
    assert_eq!(
        right,
        &[255, 0, 0, 255],
        "right half should be red, got {:?}",
        right
    );
}

/// Clear selection with undo.
#[test]
fn gpu_clear_selection_undo() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, view) = create_test_texture(&device, &queue, w, h, &red);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store =
        darkly::gpu::region_store::RegionStore::with_capacity(&device, w, h, 2 * 1024 * 1024);

    // Selection: full canvas.
    let sel_data = vec![255u8; (w * h) as usize];
    let (sel_tex, _) = create_test_texture_with_format(
        &device,
        &queue,
        w,
        h,
        &sel_data,
        wgpu::TextureFormat::R8Unorm,
    );
    let sel_view = sel_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let sel_bg = pipelines.create_selection_bind_group(&device, &sel_view, &sampler);

    // Save for undo.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Erase within selection.
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.erase_with_selection(&mut enc, &pipelines, &queue, &sel_bg);
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Verify cleared.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 32, 32, 4)[3], 0, "should be erased");

    // Undo.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 32, 32, 4),
        &[255, 0, 0, 255],
        "should be red after undo"
    );
}

/// Regression: flood fill must respect the active selection.
#[test]
fn gpu_flood_fill_respects_selection() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Transparent canvas.
    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);

    // CPU flood fill from (16, 32) on the transparent canvas — should fill everything.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let fill_mask = darkly::gpu::flood_fill::flood_fill_rgba(&pixels, w, h, 16, 32, 0);
    assert_eq!(
        fill_mask[(32 * w + 48) as usize],
        255,
        "fill mask should cover entire canvas"
    );

    // Selection: left half only (x < 32).
    let mut sel_data = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..32u32 {
            sel_data[(y * w + x) as usize] = 255;
        }
    }

    // Combine fill mask with selection (the fix being tested).
    let combined: Vec<u8> = fill_mask
        .iter()
        .zip(sel_data.iter())
        .map(|(&f, &s)| ((f as u16 * s as u16) / 255) as u8)
        .collect();

    let mask_bg =
        pipelines.upload_r8_bind_group(&device, &queue, w, h, &combined, "test-fill-sel-mask");

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.fill_rect_with_selection(
        &mut enc,
        &pipelines,
        &queue,
        [0, 0, w, h],
        [0, 0, 255, 255],
        &mask_bg,
    );
    submit(&queue, enc);

    let result = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Inside selection (left half) — should be blue.
    let inside = pixel_at(&result, w, 16, 32, 4);
    assert!(
        inside[2] > 200,
        "inside selection should be blue, B={}",
        inside[2]
    );
    assert!(
        inside[3] > 200,
        "inside selection alpha should be opaque, A={}",
        inside[3]
    );

    // Outside selection (right half) — should still be transparent.
    let outside = pixel_at(&result, w, 48, 32, 4);
    assert_eq!(
        outside[3], 0,
        "outside selection should be transparent, A={}",
        outside[3]
    );
}

// ============================================================================
// Engine-level boolean selection modes (Add / Subtract / Intersect)
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

    assert!(
        alpha_at(&pixels, w, 16, h / 2) > 0,
        "left quarter (Replace) should have paint"
    );
    assert_eq!(
        alpha_at(&pixels, w, 64, h / 2),
        0,
        "middle (not selected) should be transparent"
    );
    assert!(
        alpha_at(&pixels, w, 112, h / 2) > 0,
        "right quarter (Add) should have paint"
    );
}

/// Subtract mode: select all, subtract center band.
#[test]
fn selection_subtract_mode() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_all();
    engine.select_rect(
        48.0,
        0.0,
        32.0,
        h as f32,
        SelectionMode::Subtract,
        false,
        0.0,
    );

    paint_full_stroke(&mut engine, layer_id, w, h);
    let pixels = engine.test_readback_layer(layer_id);

    assert!(
        alpha_at(&pixels, w, 16, h / 2) > 0,
        "left (selected) should have paint"
    );
    assert_eq!(
        alpha_at(&pixels, w, 64, h / 2),
        0,
        "center (subtracted) should be transparent"
    );
    assert!(
        alpha_at(&pixels, w, 112, h / 2) > 0,
        "right (selected) should have paint"
    );
}

/// Intersect mode: left half ∩ top half = top-left quadrant only.
#[test]
fn selection_intersect_mode() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.select_rect(0.0, 0.0, 64.0, h as f32, SelectionMode::Replace, false, 0.0);
    engine.select_rect(
        0.0,
        0.0,
        w as f32,
        64.0,
        SelectionMode::Intersect,
        false,
        0.0,
    );

    // Paint horizontal strokes at two heights: y=32 (top) and y=96 (bottom).
    engine.begin_stroke(layer_id);
    for x_step in 0..20 {
        let x = x_step as f32 * (w as f32 / 20.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: 32.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();

    engine.begin_stroke(layer_id);
    for x_step in 0..20 {
        let x = x_step as f32 * (w as f32 / 20.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: 96.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 0.0,
            cg: 1.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();

    let pixels = engine.test_readback_layer(layer_id);

    // Top-left (16, 32) — in intersection.
    assert!(
        alpha_at(&pixels, w, 16, 32) > 0,
        "top-left (intersection) should have paint"
    );
    // Top-right (112, 32) — right half, outside intersection.
    assert_eq!(
        alpha_at(&pixels, w, 112, 32),
        0,
        "top-right should be transparent"
    );
    // Bottom-left (16, 96) — bottom half, outside intersection.
    assert_eq!(
        alpha_at(&pixels, w, 16, 96),
        0,
        "bottom-left should be transparent"
    );
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

    assert_eq!(
        alpha_at(&pixels, w, 16, h / 2),
        0,
        "left (inverted out) should be transparent"
    );
    assert!(
        alpha_at(&pixels, w, 112, h / 2) > 0,
        "right (inverted in) should have paint"
    );
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

    assert!(
        alpha_at(&pixels, w, w / 2, h / 2) > 0,
        "center should have paint with select_all"
    );
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
            x: 48.0,
            y: 16.0 + step as f32 * 8.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: step as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();

    let pixels = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&pixels, w, 48, 32) > 0,
        "right side should have paint after clear_selection"
    );
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
    assert!(
        !engine.has_selection(),
        "selection should be gone after undo"
    );

    // Redo — selection returns.
    engine.redo();
    assert!(
        engine.has_selection(),
        "selection should be back after redo"
    );

    // Paint with the selection active — only left half gets paint.
    paint_full_stroke(&mut engine, layer_id, w, h);
    let px = engine.test_readback_layer(layer_id);
    assert!(alpha_at(&px, w, 16, h / 2) > 0, "left should have paint");
    assert_eq!(
        alpha_at(&px, w, 112, h / 2),
        0,
        "right should be empty with selection"
    );

    // Undo the stroke, undo the selection, verify right side can be painted.
    engine.undo(); // undo stroke
    engine.undo(); // undo selection
    assert!(!engine.has_selection());

    paint_full_stroke(&mut engine, layer_id, w, h);
    let px = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&px, w, 112, h / 2) > 0,
        "right should have paint after undo (no masking)"
    );
}

/// Regression test: undoing an Add after a narrower Replace (which followed
/// a wider selection) must not restore stale pixels outside the pre-Add bounds.
///
/// The R8 scratch used for selection undo is reused across ops. A prior
/// full-canvas `save_region` would leave 1s in scratch outside the current
/// selection bounds. A subsequent Add operation would then save a tight
/// old-bounds rect into scratch but commit a full-canvas rect — picking up
/// those stale 1s and restoring them to the texture on undo.
#[test]
fn selection_add_undo_does_not_restore_stale_pixels() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    // Pollute the R8 scratch: select_all then narrow to a left band.
    // The narrow replace's save_region captures the full-canvas 1s state.
    engine.select_all();
    engine.select_rect(0.0, 0.0, 32.0, h as f32, SelectionMode::Replace, false, 0.0);

    // Add a disjoint right band (shift-modifier path).
    engine.select_rect(96.0, 0.0, 32.0, h as f32, SelectionMode::Add, false, 0.0);

    // Undo the Add — selection must revert to just the left band.
    engine.undo();

    paint_full_stroke(&mut engine, layer_id, w, h);
    let px = engine.test_readback_layer(layer_id);

    assert!(
        alpha_at(&px, w, 16, h / 2) > 0,
        "left band should still be selected after undoing the Add"
    );
    assert_eq!(
        alpha_at(&px, w, 64, h / 2),
        0,
        "middle was never selected — must stay unpainted after undo"
    );
    assert_eq!(
        alpha_at(&px, w, 112, h / 2),
        0,
        "right band was added then undone — must stay unpainted"
    );
}

// ============================================================================
// Clear selection contents (delete key)
// ============================================================================

#[test]
fn clear_selection_contents() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    // Paint at center — default brush (scale=0.1, size=0.5) at pressure=1.0
    // produces ~26px diameter dab, centered at (64,64).
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x: 64.0,
        y: 64.0,
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
    engine.render(0.0); // flush pending diff undo

    // Select left half and delete.  The dab straddles the boundary at x=64.
    engine.select_rect(0.0, 0.0, 64.0, h as f32, SelectionMode::Replace, false, 0.0);
    engine.clear_selection_contents(layer_id);

    let pixels = engine.test_readback_layer(layer_id);
    assert_eq!(
        alpha_at(&pixels, w, 56, 64),
        0,
        "left (cleared) should be transparent"
    );
    assert!(
        alpha_at(&pixels, w, 72, 64) > 0,
        "right (kept) should still have paint"
    );
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
        x: 32.0,
        y: 32.0,
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

    let pixels = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&pixels, w, 32, 32) > 0,
        "center should have paint with no selection"
    );
}

// ============================================================================
// Flat-buffer helpers (rasterize_sdf_r8, contour_segments_r8, pixel_bounds_r8)
// ============================================================================

#[test]
fn rasterize_sdf_r8_rect() {
    let (w, h) = (64u32, 64u32);
    let result = mask::rasterize_sdf_r8(
        w,
        h,
        (10, 10, 20, 20),
        |px, py| darkly::sdf::sdf_rect(px, py, 20.0, 20.0, 10.0, 10.0),
        false,
        0.0,
    );

    // The result is a tight-bounds buffer. Check a pixel inside the rect
    // relative to the region origin.
    assert!(
        result.width == 20 && result.height == 20,
        "region should be 20x20"
    );
    assert_eq!(result.x, 10);
    assert_eq!(result.y, 10);
    // Center of the shape = (20, 20) in canvas space = (10, 10) in region space.
    assert_eq!(
        result.data[(10 * result.width + 10) as usize],
        255,
        "inside rect should be 255"
    );
    // (0, 0) in region space = (10, 10) in canvas space = corner of shape, should be inside.
    assert_eq!(result.data[0], 255, "corner should be inside");
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
    assert!(
        !segments.is_empty(),
        "rectangle should produce contour segments"
    );

    let (left, right) = (rx as f32, (rx + rw) as f32);
    let (top, bottom) = (ry as f32, (ry + rh) as f32);
    let margin = 1.0;

    let on_boundary = |p: [f32; 2]| -> bool {
        let on_left =
            (p[0] - left).abs() < margin && p[1] >= top - margin && p[1] <= bottom + margin;
        let on_right =
            (p[0] - right).abs() < margin && p[1] >= top - margin && p[1] <= bottom + margin;
        let on_top = (p[1] - top).abs() < margin && p[0] >= left - margin && p[0] <= right + margin;
        let on_bottom =
            (p[1] - bottom).abs() < margin && p[0] >= left - margin && p[0] <= right + margin;
        on_left || on_right || on_top || on_bottom
    };

    for (i, (a, b)) in segments.iter().enumerate() {
        assert!(on_boundary(*a),
            "segment {i} start ({:.1}, {:.1}) not on rect boundary [{left},{top}]-[{right},{bottom}]",
            a[0], a[1]);
        assert!(
            on_boundary(*b),
            "segment {i} end ({:.1}, {:.1}) not on rect boundary [{left},{top}]-[{right},{bottom}]",
            b[0],
            b[1]
        );
    }

    // The total length of all segments should equal the rectangle perimeter.
    let total_len: f32 = segments
        .iter()
        .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
        .sum();
    let expected_perimeter = 2.0 * (rw as f32 + rh as f32);
    assert!(
        (total_len - expected_perimeter).abs() < 2.0,
        "total contour length {total_len:.1} should be ~{expected_perimeter:.1} (rect perimeter)"
    );

    // Segments should form a closed loop: for every endpoint, there must
    // be another segment with a matching endpoint (within tolerance).
    let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01;
    for (i, (a, b)) in segments.iter().enumerate() {
        for pt in [a, b] {
            let connects = segments
                .iter()
                .enumerate()
                .any(|(j, (c, d))| j != i && (close(*pt, *c) || close(*pt, *d)));
            assert!(
                connects,
                "segment {i} endpoint ({:.1}, {:.1}) is dangling (not connected)",
                pt[0], pt[1]
            );
        }
    }
}

/// Contour of a small circle: endpoints should be within the bounding circle
/// and total length should approximate the circumference.
#[test]
fn contour_segments_r8_circle_geometry() {
    let (w, h) = (128u32, 128u32);
    let (cx, cy, r) = (64.0_f32, 64.0_f32, 20.0_f32);

    let result = mask::rasterize_sdf_r8(
        w,
        h,
        (
            (cx - r) as i32,
            (cy - r) as i32,
            (2.0 * r) as i32,
            (2.0 * r) as i32,
        ),
        |px, py| darkly::sdf::sdf_circle(px, py, cx, cy, r),
        true,
        0.0,
    );
    // Expand to full canvas for contour extraction.
    let mut pixels = vec![0u8; (w * h) as usize];
    for y in 0..result.height {
        let src = (y * result.width) as usize;
        let dst = ((result.y + y) * w + result.x) as usize;
        pixels[dst..dst + result.width as usize]
            .copy_from_slice(&result.data[src..src + result.width as usize]);
    }

    let segments = mask::contour_segments_r8(&pixels, w, h, 127);
    assert!(
        !segments.is_empty(),
        "circle should produce contour segments"
    );

    // Every endpoint should be roughly at distance r from the center.
    for (i, (a, b)) in segments.iter().enumerate() {
        for pt in [a, b] {
            let dist = ((pt[0] - cx).powi(2) + (pt[1] - cy).powi(2)).sqrt();
            assert!(
                (dist - r).abs() < 2.0,
                "segment {i} endpoint ({:.1}, {:.1}) is {dist:.1} from center, expected ~{r:.0}",
                pt[0],
                pt[1]
            );
        }
    }

    // Total length should approximate circumference = 2πr.
    let total_len: f32 = segments
        .iter()
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

    assert_eq!(
        r8_segs.len(),
        tile_segs.len(),
        "r8 ({}) and tile ({}) segment counts should match",
        r8_segs.len(),
        tile_segs.len()
    );

    let eps = 0.01;
    let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps;
    for (i, r8) in r8_segs.iter().enumerate() {
        let found = tile_segs.iter().any(|t| {
            (close(r8.0, t.0) && close(r8.1, t.1)) || (close(r8.0, t.1) && close(r8.1, t.0))
        });
        assert!(
            found,
            "r8 segment {i} ({:?} -> {:?}) not in tile output",
            r8.0, r8.1
        );
    }
}
