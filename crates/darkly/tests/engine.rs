//! Engine-level GPU integration tests: brush stroke + selection, transform bounds,
//! cut/paste precision, lasso performance.
//!
//! These tests construct a real `DarklyEngine` via headless `GpuContext` and
//! exercise the same code paths that users hit.
//! Run with: `cargo test -p darkly --test engine`

use darkly::brush::wire::BrushWireType;
use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;
use darkly::nodegraph::NodeInstance;

/// Paint a solid-color brush stroke at a given position.
fn paint_at(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32, r: f32, g: f32, b: f32) {
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: r,
        cg: g,
        cb: b,
        ca: 1.0,
    });
    engine.end_stroke();
    // Flush the pending diff-based undo commit.
    engine.render(0.0);
}

/// Create a headless DarklyEngine with the given canvas dimensions.
fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Paint a horizontal brush stroke across the canvas at vertical center.
fn paint_full_stroke(engine: &mut DarklyEngine, layer_id: LayerId, w: u32, h: u32) {
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

// ============================================================================
// Brush stroke respects selection
// ============================================================================

#[test]
fn engine_brush_stroke_respects_selection() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    engine.select_rect(
        0.0,
        0.0,
        (w / 2) as f32,
        h as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    paint_full_stroke(&mut engine, layer_id, w, h);

    let pixels = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&pixels, w, w / 4, h / 2) > 0,
        "left (selected) should have paint"
    );
    assert_eq!(
        alpha_at(&pixels, w, 3 * w / 4, h / 2),
        0,
        "right (unselected) should be transparent"
    );
}

// ============================================================================
// Transform bounds are tight (pixel-level, not tile-aligned)
// ============================================================================

#[test]
fn engine_transform_bounds_are_tight() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    let sel_x = 17.0_f32;
    let sel_y = 23.0_f32;
    let sel_w = 30.0_f32;
    let sel_h = 45.0_f32;

    engine.select_rect(
        sel_x,
        sel_y,
        sel_w,
        sel_h,
        SelectionMode::Replace,
        false,
        0.0,
    );

    paint_at(
        &mut engine,
        layer_id,
        sel_x + sel_w / 2.0,
        sel_y + sel_h / 2.0,
        1.0,
        0.0,
        0.0,
    );

    let started = engine.begin_transform(layer_id);
    assert!(started, "begin_transform should succeed with a selection");

    let (origin_x, origin_y, float_w, float_h, _) = engine.floating_info().unwrap();

    assert!(
        (float_w as i32 - sel_w as i32).unsigned_abs() <= 1,
        "width should be ~{}, got {float_w}",
        sel_w as u32
    );
    assert!(
        (float_h as i32 - sel_h as i32).unsigned_abs() <= 1,
        "height should be ~{}, got {float_h}",
        sel_h as u32
    );
    assert!(
        (origin_x as i32 - sel_x as i32).abs() <= 1,
        "origin X should be ~{sel_x}, got {origin_x}"
    );
    assert!(
        (origin_y as i32 - sel_y as i32).abs() <= 1,
        "origin Y should be ~{sel_y}, got {origin_y}"
    );
}

// ============================================================================
// Paste-as-floating: cancel removes the auto-created layer
// ============================================================================

/// Regression test for the paste → transform-tool flow. `paste_image_floating`
/// auto-creates a target layer and enters floating Paste mode; cancelling
/// must remove that layer without leaving a stray undo entry.
#[test]
fn paste_floating_cancel_removes_layer() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let base_layer = engine.add_raster_layer(None);

    let pw: u32 = 8;
    let ph: u32 = 8;
    let rgba = vec![0xFFu8; (pw * ph * 4) as usize];

    let pasted_id = engine.paste_image_floating(pw, ph, &rgba, 10, 10, Some(base_layer));

    assert!(
        engine.has_layer(pasted_id),
        "auto-created paste layer should exist after paste_image_floating"
    );
    assert!(
        engine.has_floating(),
        "should be in floating mode after paste_image_floating"
    );

    engine.cancel_floating();

    assert!(
        !engine.has_floating(),
        "floating should be cleared after cancel"
    );
    assert!(
        !engine.has_layer(pasted_id),
        "auto-created paste layer should be removed after cancel"
    );
    assert!(
        engine.has_layer(base_layer),
        "pre-existing layer must remain after cancel"
    );

    engine.undo();
    assert!(
        !engine.has_layer(pasted_id),
        "undo after cancel must not resurrect the pasted layer"
    );
}

/// Regression: `begin_transform` on a layer whose bounds extend past the
/// canvas (e.g. just-committed oversized paste, no selection) must:
///   1. compute content bounds over the layer texture's full extent (not
///      just canvas-sized top-left), and
///   2. translate those layer-local bounds into canvas-space before
///      handing them to `setup_transform`, so save_region/clear/restore
///      land on the correct slice of the layer texture.
///
/// Bug symptoms before fix: floating preview snapped to canvas (0, 0),
/// only the canvas-sized top-left of the texture was transformed, and
/// cancel destructively cleared the canvas-aligned region of the layer.
#[test]
fn transform_on_off_canvas_layer_cancel_restores_pixels() {
    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    // 128×128 opaque red, centered: layer bounds (-32, -32, 128, 128).
    let pw: u32 = 128;
    let ph: u32 = 128;
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 255;
        px[3] = 255;
    }
    let pasted_id = engine.paste_image(pw, ph, &rgba, -32, -32, None);

    let before = engine.test_readback_layer(pasted_id);

    // No selection — drives the async content_bounds compute path.
    // First call dispatches; subsequent frames complete the readback.
    let started = engine.begin_transform(pasted_id);
    assert!(
        !started,
        "no-selection path should defer for content_bounds"
    );

    // Drive readbacks to completion. `test_flush_readbacks` polls Wait,
    // which also flushes content_bounds map_async callbacks.
    let mut floating_ready = false;
    for _ in 0..16 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if engine.has_floating() {
            floating_ready = true;
            break;
        }
    }
    assert!(
        floating_ready,
        "begin_transform did not resolve within 16 iterations"
    );

    // The floating must report the layer's full extent in canvas-space.
    let (ox, oy, fw, fh, _) = engine.floating_info().expect("floating info");
    assert_eq!(
        (ox as i32, oy as i32),
        (-32, -32),
        "source_origin should be canvas-space (layer offset), not layer-local (0,0)"
    );
    assert_eq!(fw as u32, pw);
    assert_eq!(fh as u32, ph);

    // Cancel must restore byte-identical layer pixels — including the
    // off-canvas region that lives outside `[0, 0, canvas_w, canvas_h]`.
    engine.cancel_floating();

    let after = engine.test_readback_layer(pasted_id);
    assert_eq!(
        before, after,
        "layer pixels must be byte-identical after cancel"
    );
}

/// Regression: committing a floating transform that translates content past
/// the canvas edge must grow the target layer to fit the moved pixels,
/// rather than clamping the affected rect to canvas bounds and silently
/// dropping anything outside it.
///
/// Bug symptom before fix: a translated selection whose new bounds extend
/// past the canvas edge would be cropped at the canvas boundary on commit —
/// pixels beyond the edge were never written to the target layer texture.
#[test]
fn commit_floating_translate_past_canvas_preserves_pixels() {
    use darkly::coord::CanvasRect;
    use darkly::gpu::transform::affine_translate;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    // Opaque red 8×8 block at canvas (50, 30) — fully inside the canvas.
    let bw: u32 = 8;
    let bh: u32 = 8;
    let mut rgba = vec![0u8; (bw * bh * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 255;
        px[3] = 255;
    }
    let red_layer = engine.paste_image(bw, bh, &rgba, 50, 30, None);
    assert_eq!(
        engine.layer_bounds(red_layer),
        Some(CanvasRect::from_xywh(50, 30, bw, bh)),
        "pasted layer should start sized to the paste"
    );

    // Select the block; transform it.
    engine.select_rect(
        50.0,
        30.0,
        bw as f32,
        bh as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    let started = engine.begin_transform(red_layer);
    assert!(
        started,
        "begin_transform with a selection should be synchronous"
    );

    // Translate the floating by +20 in X — transformed bounds (70, 30, 8, 8)
    // sit entirely past the canvas right edge (canvas width = 64).
    engine.update_floating_matrix(affine_translate(20.0, 0.0));
    engine.commit_floating();

    // Layer must have grown to contain the translated rect.
    let bounds = engine
        .layer_bounds(red_layer)
        .expect("layer must still exist after commit");
    assert!(
        bounds.contains(CanvasRect::from_xywh(70, 30, bw, bh)),
        "layer bounds must contain the off-canvas translated rect (70, 30, 8, 8); got {:?}",
        bounds
    );

    // Verify the moved red is actually written into the grown texture at
    // the right canvas-space location. This is what the bug was dropping.
    let pixels = engine.test_readback_layer(red_layer);
    let lw = bounds.width;
    let lx = (70 - bounds.x0()) as u32;
    let ly = (30 - bounds.y0()) as u32;
    let p_idx = ((ly * lw + lx) * 4) as usize;
    assert!(
        pixels[p_idx] > 200,
        "moved pixel at canvas (70, 30) should be red, got R={}",
        pixels[p_idx]
    );
    assert!(
        pixels[p_idx + 3] > 200,
        "moved pixel at canvas (70, 30) should be opaque, got A={}",
        pixels[p_idx + 3]
    );
}

/// Regression for the canvas-clamping bug: pasting an image larger than
/// the canvas must preserve the full extent on the layer, not crop to
/// canvas dimensions.
#[test]
fn paste_image_floating_preserves_off_canvas_extent() {
    use darkly::coord::CanvasRect;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    // 4× wider than canvas, 4× taller.
    let pw: u32 = 256;
    let ph: u32 = 256;
    let rgba = vec![0x88u8; (pw * ph * 4) as usize];

    // Center on canvas — paste extent goes from (-96, -96) to (160, 160).
    let ox = (cw as i32 - pw as i32) / 2;
    let oy = (ch as i32 - ph as i32) / 2;
    let pasted_id = engine.paste_image_floating(pw, ph, &rgba, ox, oy, None);

    let bounds = engine
        .layer_bounds(pasted_id)
        .expect("pasted layer must have bounds");
    assert_eq!(
        bounds,
        CanvasRect::from_xywh(ox, oy, pw, ph),
        "layer bounds must match the full paste extent"
    );

    engine.commit_floating();

    // Bounds survive commit — the layer texture still has the full
    // off-canvas extent, even though the visible canvas only intersects
    // the centered 64×64 region.
    let bounds = engine
        .layer_bounds(pasted_id)
        .expect("pasted layer still exists after commit");
    assert_eq!(bounds.width, pw);
    assert_eq!(bounds.height, ph);
}

/// Same guarantee for the non-floating direct paste path (`paste_image`).
#[test]
fn paste_image_direct_preserves_off_canvas_extent() {
    use darkly::coord::CanvasRect;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    let pw: u32 = 200;
    let ph: u32 = 100;
    let rgba = vec![0x44u8; (pw * ph * 4) as usize];

    let pasted_id = engine.paste_image(pw, ph, &rgba, -50, 10, None);

    let bounds = engine
        .layer_bounds(pasted_id)
        .expect("pasted layer must have bounds");
    assert_eq!(
        bounds,
        CanvasRect::from_xywh(-50, 10, pw, ph),
        "direct paste layer bounds must match the full paste extent"
    );
}

/// Regression: `floating_target_layer` returns the auto-created layer for
/// a paste-as-floating, so the frontend can distinguish "user switched away
/// from floating's layer" from "user just activated floating's own target".
#[test]
fn paste_floating_target_layer_matches_created() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let base_layer = engine.add_raster_layer(None);

    assert_eq!(
        engine.floating_target_layer(),
        None,
        "no floating, no target"
    );

    let pw: u32 = 8;
    let ph: u32 = 8;
    let rgba = vec![0xFFu8; (pw * ph * 4) as usize];
    let pasted_id = engine.paste_image_floating(pw, ph, &rgba, 10, 10, Some(base_layer));

    assert_eq!(
        engine.floating_target_layer(),
        Some(pasted_id),
        "floating_target_layer must match the pasted layer id"
    );

    engine.cancel_floating();
    assert_eq!(
        engine.floating_target_layer(),
        None,
        "no target after cancel"
    );
}

/// Companion: committing a floating paste keeps the layer and registers
/// exactly one undoable LayerAddAction (so a single undo removes the paste).
#[test]
fn paste_floating_commit_is_one_undo() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let base_layer = engine.add_raster_layer(None);

    let pw: u32 = 8;
    let ph: u32 = 8;
    let rgba = vec![0xFFu8; (pw * ph * 4) as usize];

    let pasted_id = engine.paste_image_floating(pw, ph, &rgba, 10, 10, Some(base_layer));
    engine.commit_floating();

    assert!(engine.has_layer(pasted_id), "pasted layer should remain");
    assert!(!engine.has_floating(), "floating cleared after commit");

    engine.undo();
    assert!(
        !engine.has_layer(pasted_id),
        "single undo must remove the pasted layer entirely"
    );
}

/// Regression: a paste smaller than canvas in any dimension produces a
/// paste-extent layer texture smaller than the canvas. The thumbnail
/// readback must source from the texture's actual dimensions — copying
/// `[0, 0, canvas_w, canvas_h]` exceeds the texture and fails wgpu
/// validation, invalidating the entire command encoder.
#[test]
fn thumbnail_readback_handles_layer_smaller_than_canvas() {
    let (cw, ch) = (1920, 1080);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    // 256×256 paste — both dims smaller than canvas.
    let pw: u32 = 256;
    let ph: u32 = 256;
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[1] = 200;
        px[3] = 255;
    }
    let pasted = engine.paste_image(pw, ph, &rgba, 100, 100, None);

    // Render drives `drain_dirty_thumbnail_readbacks` which encodes the
    // copy. Pre-fix this submits an invalid CommandEncoder ("touches outside
    // of layer-texture") and poisons the queue.
    engine.render(0.0);
    for _ in 0..8 {
        engine.test_flush_readbacks();
        if engine.test_thumbnail_cache_peek(pasted).is_some() {
            break;
        }
    }

    let thumb = engine
        .test_thumbnail_cache_peek(pasted)
        .expect("thumbnail must complete after a few flushes");
    let any_visible = thumb.chunks_exact(4).any(|p| p[3] > 0);
    assert!(
        any_visible,
        "thumbnail of a non-empty paste must contain visible pixels"
    );
}

/// Regression: the floating preview after `paste_image_floating` must be
/// populated immediately. Pre-fix, `set_floating_content` allocated the
/// preview texture but never wrote into it; the host blend pass sampled
/// from an uninitialized texture, so the paste appeared invisible until
/// the first drag triggered `update_floating_matrix` →
/// `update_floating_preview`.
#[test]
fn paste_image_floating_preview_visible_before_any_drag() {
    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    // 32×32 opaque green paste centered on canvas.
    let pw: u32 = 32;
    let ph: u32 = 32;
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[1] = 255;
        px[3] = 255;
    }
    let ox = (cw as i32 - pw as i32) / 2;
    let oy = (ch as i32 - ph as i32) / 2;
    let _pasted = engine.paste_image_floating(pw, ph, &rgba, ox, oy, None);

    // No `update_floating_matrix` call — just composite once and read the
    // canvas back. The paste must already be visible.
    let canvas = engine.test_readback_canvas();
    let center_x = (cw / 2) as usize;
    let center_y = (ch / 2) as usize;
    let p_idx = (center_y * cw as usize + center_x) * 4;
    assert!(
        canvas[p_idx + 1] > 200,
        "center pixel must be green from the paste preview, got G={}",
        canvas[p_idx + 1]
    );
    assert!(
        canvas[p_idx + 3] > 200,
        "center pixel must be opaque, got A={}",
        canvas[p_idx + 3]
    );
}

/// Regression: while dragging a floating, the preview must show the
/// transformed content at the new canvas position — not clip it to the
/// floating's source bounding box.
///
/// Pre-fix, the preview texture was allocated at the live layer's
/// dimensions (which for a paste = the source's bounding box) and reused
/// the live layer's blend uniforms. Translating the matrix moved the
/// transform-shader's write outside the preview texture, so the host
/// blend pass sampled the still-empty parts of the preview at the new
/// destination — the moved content was invisible until commit.
#[test]
fn floating_preview_visible_when_translation_extends_past_source_bbox() {
    use darkly::gpu::transform::affine_translate;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    // 8×8 opaque red paste at canvas (10, 10). Source bbox = (10, 10, 8, 8).
    let bw: u32 = 8;
    let bh: u32 = 8;
    let mut rgba = vec![0u8; (bw * bh * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 255;
        px[3] = 255;
    }
    let _pasted = engine.paste_image_floating(bw, bh, &rgba, 10, 10, None);

    // Translate by +20 in X → transformed bounds (30, 10, 8, 8), well
    // outside the source bbox at (10, 10, 8, 8) but still inside canvas.
    engine.update_floating_matrix(affine_translate(20.0, 0.0));

    let canvas = engine.test_readback_canvas();
    let pixel_at = |x: u32, y: u32| {
        let i = ((y * cw + x) * 4) as usize;
        [canvas[i], canvas[i + 1], canvas[i + 2], canvas[i + 3]]
    };

    // Moved content must be visible at (30..38, 10..18).
    let moved = pixel_at(34, 14);
    assert!(
        moved[0] > 200 && moved[3] > 200,
        "moved pixel at canvas (34, 14) should be opaque red, got {:?}",
        moved
    );

    // Old position must be cleared.
    let old = pixel_at(14, 14);
    assert!(
        old[3] < 50,
        "old position (14, 14) should be transparent during transform, got A={}",
        old[3]
    );
}

/// Regression: dragging the floating must not leave ghost copies of the
/// content at the previous frame's destination. The canvas-aligned preview
/// is a long-lived texture; each `update_floating_matrix` overwrites the
/// transformed region, but pixels at the *previous* destination must be
/// reset — otherwise the shader's "discard outside transformed bounds"
/// leaves the old pixels in place, building a smear across the drag path.
#[test]
fn floating_preview_does_not_leave_ghost_pixels_when_dragged() {
    use darkly::gpu::transform::affine_translate;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    let bw: u32 = 8;
    let bh: u32 = 8;
    let mut rgba = vec![0u8; (bw * bh * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 255;
        px[3] = 255;
    }
    let _pasted = engine.paste_image_floating(bw, bh, &rgba, 10, 10, None);

    // First drag: move to (+15, 0) → (25, 10, 8, 8).
    engine.update_floating_matrix(affine_translate(15.0, 0.0));
    let _ = engine.test_readback_canvas();

    // Second drag: move further to (+30, 0) → (40, 10, 8, 8).
    engine.update_floating_matrix(affine_translate(30.0, 0.0));
    let canvas = engine.test_readback_canvas();

    let pixel_at = |x: u32, y: u32| {
        let i = ((y * cw + x) * 4) as usize;
        canvas[i + 3]
    };

    // Current position must be visible.
    assert!(
        pixel_at(44, 14) > 200,
        "current drag position (44, 14) must be opaque, got A={}",
        pixel_at(44, 14)
    );
    // Previous-drag destination (25..33, 10..18) must NOT still hold pixels.
    assert!(
        pixel_at(29, 14) < 50,
        "previous-drag position (29, 14) must NOT retain pixels, got A={}",
        pixel_at(29, 14)
    );
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
    let layer_id = engine.add_raster_layer(None);

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
    assert!(
        ms < 50.0,
        "select_lasso with {n_verts} verts took {ms:.1}ms, expected <50ms"
    );

    assert!(engine.has_selection());

    // Correctness: paint across canvas, verify masking works.
    engine.begin_stroke(layer_id);
    for x_step in 0..40 {
        let x = x_step as f32 * (w as f32 / 40.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: cy,
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

    let pixels = engine.test_readback_layer(layer_id);

    // Center of polygon (500, 500) — should have paint.
    assert!(
        alpha_at(&pixels, w, cx as u32, cy as u32) > 0,
        "center of lasso should have paint"
    );

    // Well outside polygon (50, 500) — 450px left of center, outside r=200.
    assert_eq!(
        alpha_at(&pixels, w, 50, cy as u32),
        0,
        "outside lasso should be transparent"
    );
}

fn find_node_id(engine: &DarklyEngine, type_id: &str) -> u64 {
    engine
        .active_brush_graph()
        .nodes
        .values()
        .find(|n: &&NodeInstance<BrushWireType>| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in default graph"))
        .id
        .0
}

// ============================================================================
// pen_input.spacing port controls dab spacing
// ============================================================================

/// Sum of alpha across the canvas — proxy for "amount of paint deposited."
fn alpha_sum(pixels: &[u8], w: u32, h: u32) -> u64 {
    let mut s: u64 = 0;
    for y in 0..h {
        for x in 0..w {
            s += alpha_at(pixels, w, x, y) as u64;
        }
    }
    s
}

fn paint_horizontal_stroke(engine: &mut DarklyEngine, layer_id: LayerId, w: u32, h: u32) {
    engine.begin_stroke(layer_id);
    let samples = 40;
    for i in 0..samples {
        let t = i as f32 / (samples - 1) as f32;
        let x = 16.0 + t * (w as f32 - 32.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: (h / 2) as f32,
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
    engine.render(0.0);
}

/// Setting `pen_input.spacing` to a larger ratio drops fewer dabs along the
/// stroke, so total deposited alpha is lower than at the default 10%.
/// Guards the wiring from `pen_input.spacing` port → `SpacingConfig.ratio`.
#[test]
fn pen_input_spacing_port_controls_dab_density() {
    let (w, h) = (256, 256);

    // Baseline: default spacing (port default = 0.10).
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    let pen_id = find_node_id(&engine, "pen_input");
    engine
        .brush_graph_set_port_default(pen_id, "spacing", 0.10)
        .expect("default spacing port must exist");
    paint_horizontal_stroke(&mut engine, layer_id, w, h);
    let dense_alpha = alpha_sum(&engine.test_readback_layer(layer_id), w, h);

    // Sparse: 100% spacing — dabs separated by a full diameter.
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    let pen_id = find_node_id(&engine, "pen_input");
    engine
        .brush_graph_set_port_default(pen_id, "spacing", 1.0)
        .expect("spacing port must exist");
    paint_horizontal_stroke(&mut engine, layer_id, w, h);
    let sparse_alpha = alpha_sum(&engine.test_readback_layer(layer_id), w, h);

    // 100% spacing (dabs separated by a full diameter) means each pixel
    // is touched by at most ~1 soft dab, vs. ~10× overlap at 10%. Soft
    // tips with falloff don't yield a 10× alpha ratio (each pixel saturates),
    // but the difference is comfortably more than 25%.
    assert!(
        sparse_alpha * 4 < dense_alpha * 3,
        "expected 100% spacing to deposit noticeably less paint than 10%; \
         got dense={dense_alpha}, sparse={sparse_alpha} (sparse/dense = {:.2})",
        sparse_alpha as f64 / dense_alpha as f64
    );
}

/// Regression: at the smallest brush sizes, `SpacingConfig::distance()`
/// previously relied on `min_px` defaulting to 1.0 to avoid sub-pixel
/// dab stepping. If any code path constructed a `SpacingConfig` with
/// `min_px < 1.0` — or a future change scaled spacing without going
/// through `SpacingConfig::distance()` — strokes with a tiny brush would
/// emit one dab per *fractional* pixel of stroke, producing catastrophic
/// dab counts. Guard the invariant end-to-end: a long stroke painted
/// with the smallest brush must not place more dabs than the stroke is
/// pixels long.
#[test]
fn small_brush_does_not_emit_subpixel_dab_spacing() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Force the densest configuration the UI allows:
    // - Spacing ratio at its 4 % floor (any lower swamps the stabilizer).
    // - Stamp size at near-zero, so the dab is clamped to 1×1 px and
    //   the per-iteration step would be `1 * 0.04 = 0.04 px` without
    //   the absolute floor.
    let pen_id = find_node_id(&engine, "pen_input");
    engine
        .brush_graph_set_port_default(pen_id, "spacing", 0.04)
        .expect("spacing port must exist");
    let stamp_id = find_node_id(&engine, "stamp");
    engine
        .brush_graph_set_port_default(stamp_id, "size", 0.001)
        .expect("stamp size port must exist");

    // Horizontal stroke from x=16 to x=(w-16) at y = h/2. Same shape
    // as `paint_horizontal_stroke`, repeated here so the stroke length
    // is explicit at the assertion site.
    let x0 = 16.0_f32;
    let x1 = (w as f32) - 16.0;
    let stroke_length_px = (x1 - x0).abs();

    engine.begin_stroke(layer_id);
    let samples = 40;
    for i in 0..samples {
        let t = i as f32 / (samples - 1) as f32;
        let x = x0 + t * (x1 - x0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: (h / 2) as f32,
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
    engine.render(0.0);

    let dabs = engine.test_stroke_total_dabs();
    // The 1 px floor caps *fresh* dab placements at one per stroke pixel.
    // `total_dabs` also counts dabs re-placed by the tip-divergence re-render
    // (every pen event re-renders the tip segment with proper Catmull-Rom
    // lookahead) and any checkpoint-restore replay, so the observed total is
    // higher than the stroke length even when the floor holds.
    //
    // The bound below is the gross-regression guard: if `SpacingConfig::distance()`
    // ever returned a sub-pixel value (e.g. 0.5 px), this number would roughly
    // double; if it returned 0.1 px, it'd grow ~10×. The companion
    // `debug_assert!(step >= ABSOLUTE_MIN_SPACING_PX, …)` in
    // `stroke_engine::render_from_stabilized_*` is the precise per-step guard
    // and would trip first under cargo test (which runs with debug_assertions).
    let max_expected = (stroke_length_px.ceil() as u64) * 4;
    assert!(
        dabs <= max_expected,
        "tiny-brush stroke emitted {dabs} dabs across {stroke_length_px:.0}px \
         of stroke (gross-regression bound {max_expected}); spacing floor \
         appears to have been bypassed"
    );
}

/// Brush stroke on a paste-extent layer (offset, larger than canvas) +
/// undo: the layer texture must be byte-identical to its pre-stroke state
/// after undo, including off-canvas pixels that were unaffected.
/// Regression for P1d (StrokeBuffer sized to layer bounds, not canvas).
#[test]
fn brush_stroke_on_paste_extent_layer_undo_preserves_off_canvas_pixels() {
    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);

    // Paste a 200×200 image at (-50, -50). Layer canvas extent is
    // (-50..150, -50..150) — mostly off-canvas in both directions.
    let pw: u32 = 200;
    let ph: u32 = 200;
    // Distinct off-canvas marker: solid blue with high alpha.
    let rgba: Vec<u8> = (0..pw * ph).flat_map(|_| [10u8, 20, 200, 255]).collect();
    let pasted_id = engine.paste_image(pw, ph, &rgba, -50, -50, None);

    let pre_stroke = engine.test_readback_layer(pasted_id);
    assert_eq!(pre_stroke.len(), (pw * ph * 4) as usize);

    // Paint a stroke at canvas (10, 10) — that's layer-local (60, 60).
    paint_at(&mut engine, pasted_id, 10.0, 10.0, 1.0, 0.0, 0.0);

    let after_stroke = engine.test_readback_layer(pasted_id);
    assert_ne!(
        pre_stroke, after_stroke,
        "stroke should have changed at least one pixel"
    );

    engine.undo();
    engine.render(0.0);

    let after_undo = engine.test_readback_layer(pasted_id);
    assert_eq!(
        pre_stroke, after_undo,
        "undo on paste-extent layer must restore byte-identical pre-stroke pixels (including off-canvas)"
    );
}

/// Brush stroke at a canvas position on a paste-extent layer with negative
/// offset must land at the corresponding layer-local position, not at
/// canvas-pos interpreted as layer-local.
#[test]
fn brush_stroke_on_paste_extent_layer_lands_at_canvas_coords() {
    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);

    let pw: u32 = 200;
    let ph: u32 = 200;
    let rgba = vec![0u8; (pw * ph * 4) as usize]; // transparent
    let off_x = -50;
    let off_y = -50;
    let pasted_id = engine.paste_image(pw, ph, &rgba, off_x, off_y, None);

    // Paint at canvas (10, 10) — layer-local (60, 60).
    paint_at(&mut engine, pasted_id, 10.0, 10.0, 1.0, 0.0, 0.0);

    let pixels = engine.test_readback_layer(pasted_id);
    let lx = (10 - off_x) as u32;
    let ly = (10 - off_y) as u32;

    // The stroke center must have non-zero alpha at the expected layer-local
    // coords (60, 60). Use a small search box because brush dabs may not
    // hit the exact center pixel depending on rendering details.
    let mut hit = false;
    for dy in 0..6u32 {
        for dx in 0..6u32 {
            let px = lx.saturating_sub(3) + dx;
            let py = ly.saturating_sub(3) + dy;
            if alpha_at(&pixels, pw, px, py) > 0 {
                hit = true;
                break;
            }
        }
    }
    assert!(
        hit,
        "stroke must land at layer-local ({lx}, {ly}) — canvas-space coords expected"
    );

    // The OLD bug placed strokes at layer-local (10, 10) — canvas coords
    // interpreted as layer-local. That region must be untouched.
    let mut wrong_hit = 0u32;
    for dy in 0..6u32 {
        for dx in 0..6u32 {
            let px = (10u32).saturating_sub(3) + dx;
            let py = (10u32).saturating_sub(3) + dy;
            wrong_hit = wrong_hit.max(alpha_at(&pixels, pw, px, py) as u32);
        }
    }
    assert_eq!(
        wrong_hit, 0,
        "layer-local (10, 10) area should be untouched (would be wrong-place stroke)"
    );
}

// ============================================================================
// Brush strokes grow the layer
// ============================================================================

/// Brush stroke whose center falls past the canvas right edge must extend
/// the layer's canvas extent rightward by at least one growth chunk
/// (256-pixel multiple), preserving the originally-allocated content.
#[test]
fn brush_stroke_off_canvas_grows_layer() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    let bounds_before = engine.layer_bounds(layer_id).expect("layer exists");
    assert_eq!(bounds_before.origin.x, 0);
    assert_eq!(bounds_before.origin.y, 0);
    assert_eq!(bounds_before.width, cw);
    assert_eq!(bounds_before.height, ch);

    // Paint at canvas (cw + 50, ch / 2) — well past the right edge.
    paint_at(
        &mut engine,
        layer_id,
        cw as f32 + 50.0,
        ch as f32 / 2.0,
        1.0,
        0.0,
        0.0,
    );

    let bounds_after = engine.layer_bounds(layer_id).expect("layer still exists");
    assert!(
        bounds_after.width > cw,
        "layer width should have grown past canvas; before {}, after {}",
        cw,
        bounds_after.width,
    );
    assert_eq!(
        bounds_after.origin.x, 0,
        "positive-direction growth should keep origin at 0"
    );
}

/// After a stroke off the canvas right edge grows the layer, the painted
/// pixel must land at the canvas-space position requested — i.e. at the
/// layer-local position `(canvas_x - layer_offset_x, canvas_y - layer_offset_y)`.
#[test]
fn brush_stroke_off_canvas_pixel_lands_correctly() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    let canvas_x: i32 = cw as i32 + 80;
    let canvas_y: i32 = ch as i32 / 2;
    paint_at(
        &mut engine,
        layer_id,
        canvas_x as f32,
        canvas_y as f32,
        1.0,
        0.0,
        0.0,
    );

    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    let pixels = engine.test_readback_layer(layer_id);
    assert_eq!(
        pixels.len(),
        (bounds.width * bounds.height * 4) as usize,
        "readback should match grown layer dimensions"
    );

    let lx = (canvas_x - bounds.origin.x) as u32;
    let ly = (canvas_y - bounds.origin.y) as u32;
    // The brush dab's actual radius depends on the active brush graph, so
    // search a generous box around the expected layer-local center to
    // accommodate dabs of different sizes.
    let half: u32 = 64;
    let mut hit = false;
    'outer: for dy in 0..(half * 2) {
        for dx in 0..(half * 2) {
            let px = lx.saturating_sub(half) + dx;
            let py = ly.saturating_sub(half) + dy;
            if px < bounds.width
                && py < bounds.height
                && alpha_at(&pixels, bounds.width, px, py) > 0
            {
                hit = true;
                break 'outer;
            }
        }
    }
    assert!(
        hit,
        "off-canvas paint at canvas ({canvas_x}, {canvas_y}) should land at layer-local ({lx}, {ly})"
    );
}

/// Negative-direction growth on the X axis: a dab at canvas (-100, h/2)
/// must shift the layer's `offset_x` more negative by at least one chunk
/// (256), expand the width to cover, and preserve the original content.
#[test]
fn layer_growth_negative_direction() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    paint_at(
        &mut engine,
        layer_id,
        -100.0,
        ch as f32 / 2.0,
        0.0,
        1.0,
        0.0,
    );

    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    assert!(
        bounds.origin.x <= -256,
        "negative-direction growth should shift offset_x by at least one chunk; got {}",
        bounds.origin.x
    );
    assert!(
        bounds.width >= cw + 256,
        "width should expand to cover the new origin shift; got {}",
        bounds.width
    );
}

/// Negative-direction growth on the Y axis: same as above but for Y.
#[test]
fn layer_growth_negative_direction_y() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    paint_at(
        &mut engine,
        layer_id,
        cw as f32 / 2.0,
        -100.0,
        0.0,
        0.0,
        1.0,
    );

    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    assert!(
        bounds.origin.y <= -256,
        "negative-direction Y growth should shift offset_y by at least one chunk; got {}",
        bounds.origin.y
    );
    assert!(
        bounds.height >= ch + 256,
        "height should expand to cover the new origin shift; got {}",
        bounds.height
    );
}

/// A dab one pixel past the canvas right edge must grow the layer width
/// to at least one full chunk past the canvas — not just one extra pixel.
/// Confirms `round_outward(LAYER_GROWTH_CHUNK)` is applied to grown bounds.
#[test]
fn layer_growth_chunked_to_256() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Just one pixel past the right edge.
    paint_at(
        &mut engine,
        layer_id,
        cw as f32 + 1.0,
        ch as f32 / 2.0,
        1.0,
        0.0,
        0.0,
    );

    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    assert!(
        bounds.width >= cw + 256,
        "1-pixel overshoot should still snap to a full chunk: width={}",
        bounds.width
    );
    // Grown width should be a multiple of 256.
    assert_eq!(
        bounds.width % 256,
        0,
        "width should be chunk-aligned: {}",
        bounds.width
    );
}

/// A stroke that grows the layer can be undone, restoring pre-stroke
/// pixels in the original layer extent. Pixels in the newly-grown region
/// were transparent before the stroke (didn't exist in the layer), and
/// are transparent again after undo.
#[test]
fn undo_after_growth_restores_pixels_in_old_bounds() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Pre-stroke: fill a known canvas-aligned region so we can confirm
    // it's restored byte-for-byte after undo.
    paint_at(&mut engine, layer_id, 64.0, 64.0, 1.0, 0.0, 0.0);

    let pre_stroke = engine.test_readback_layer(layer_id);
    let pre_bounds = engine.layer_bounds(layer_id).unwrap();

    // Now paint past the right edge — this triggers growth.
    paint_at(
        &mut engine,
        layer_id,
        cw as f32 + 80.0,
        ch as f32 / 2.0,
        0.0,
        1.0,
        0.0,
    );
    let grown_bounds = engine.layer_bounds(layer_id).unwrap();
    assert!(
        grown_bounds.width > pre_bounds.width,
        "layer should have grown"
    );

    engine.undo();
    engine.render(0.0);

    let after_undo = engine.test_readback_layer(layer_id);
    let after_bounds = engine.layer_bounds(layer_id).unwrap();
    // After undo the layer extent stays at its grown size (we don't shrink
    // on undo; the polish step is a deferred follow-up).
    assert_eq!(after_bounds, grown_bounds, "undo doesn't shrink bounds");

    // Compare the OLD canvas-aligned region — must match the pre-stroke
    // byte sequence. We sample a strip at y=64 across the full original
    // width to keep the assertion fast and informative.
    for x in 0..pre_bounds.width {
        let pre_idx = (((64) * pre_bounds.width + x) * 4) as usize;
        let new_x = x as i32 + (pre_bounds.origin.x - after_bounds.origin.x);
        let new_y = 64i32 + (pre_bounds.origin.y - after_bounds.origin.y);
        if new_x < 0 || new_y < 0 {
            continue;
        }
        let cur_idx = (((new_y as u32) * after_bounds.width + new_x as u32) * 4) as usize;
        assert_eq!(
            &pre_stroke[pre_idx..pre_idx + 4],
            &after_undo[cur_idx..cur_idx + 4],
            "row 64 col {x}: pre-stroke pixels in the old bounds must be restored after undo"
        );
    }
}

/// Growth past the `MAX_LAYER_DIM` cap is refused: the dab is silently
/// clipped to current bounds, the layer's bounds stay below the cap, and
/// no panic occurs.
#[test]
fn layer_growth_capped_at_max() {
    use darkly::gpu::compositor::MAX_LAYER_DIM;
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Paint far enough out to push past the cap. MAX_LAYER_DIM is 16384.
    paint_at(
        &mut engine,
        layer_id,
        (MAX_LAYER_DIM as f32) + 1000.0,
        ch as f32 / 2.0,
        1.0,
        0.0,
        0.0,
    );

    let bounds = engine.layer_bounds(layer_id).unwrap();
    assert!(
        bounds.width <= MAX_LAYER_DIM,
        "layer width must stay within MAX_LAYER_DIM; got {}",
        bounds.width
    );
    assert!(
        bounds.height <= MAX_LAYER_DIM,
        "layer height must stay within MAX_LAYER_DIM; got {}",
        bounds.height
    );
}

/// A long stroke that crosses the canvas boundary mid-stroke triggers
/// growth between dabs; the saved pre-stroke region must remain valid
/// after the grow so undo restores the originally-painted pre-stroke
/// content (canvas-anchored), not random scratch garbage.
#[test]
fn mid_stroke_growth_preserves_already_saved_region() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Pre-paint distinctive canvas-aligned content so we have a baseline.
    paint_at(&mut engine, layer_id, 100.0, 100.0, 1.0, 0.0, 0.0);
    let pre_stroke_bounds = engine.layer_bounds(layer_id).unwrap();
    let pre_stroke = engine.test_readback_layer(layer_id);

    // Now do a single stroke composed of multiple events, crossing the
    // canvas right edge. The first event is in-canvas; later events
    // trigger grow.
    engine.begin_stroke(layer_id);
    for x_step in 0..10 {
        let x = (cw as f32) * 0.4 + (x_step as f32) * 80.0;
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: ch as f32 / 2.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: x_step as f64 * 16.0,
            cr: 0.0,
            cg: 0.0,
            cb: 1.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
    engine.render(0.0);

    let grown_bounds = engine.layer_bounds(layer_id).unwrap();
    assert!(
        grown_bounds.width > pre_stroke_bounds.width,
        "stroke should have grown the layer"
    );

    engine.undo();
    engine.render(0.0);

    let after_undo = engine.test_readback_layer(layer_id);
    let after_bounds = engine.layer_bounds(layer_id).unwrap();
    // Pre-stroke pixel at canvas (100, 100) was red — confirm it's
    // restored at the corresponding layer-local position.
    let lx = (100 - after_bounds.origin.x) as u32;
    let ly = (100 - after_bounds.origin.y) as u32;
    let mut found_red = false;
    for dy in 0..8u32 {
        for dx in 0..8u32 {
            let px = lx.saturating_sub(4) + dx;
            let py = ly.saturating_sub(4) + dy;
            if px < after_bounds.width && py < after_bounds.height {
                let idx = ((py * after_bounds.width + px) * 4) as usize;
                if after_undo[idx] > 200 && after_undo[idx + 3] > 200 {
                    found_red = true;
                    break;
                }
            }
        }
    }
    let _ = pre_stroke; // kept for potential future byte-exact comparison
    assert!(
        found_red,
        "after-undo: pre-stroke red pixels at canvas (100, 100) must survive mid-stroke grow"
    );
}

/// `LayerInfo::Raster` carries the layer's canvas-space bounds so the
/// frontend can see paste-extent storage. Regression for P4: a layer
/// whose bounds extend past the canvas (paste of an oversized image)
/// reports those exact bounds through the FFI-facing `LayerInfo`, and
/// the `serde` round-trip preserves them.
#[test]
fn layer_info_carries_paste_extent_bounds_through_serde() {
    use darkly::coord::CanvasRect;
    use darkly::engine::types::LayerInfo;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    // Paste 200×200 at (-50, -50) — paste-extent layer with bounds that
    // extend in both negative-canvas directions and past the canvas.
    let pw: u32 = 200;
    let ph: u32 = 200;
    let rgba = vec![0x33u8; (pw * ph * 4) as usize];
    let pasted_id = engine.paste_image(pw, ph, &rgba, -50, -50, None);

    // Walk the engine's layer tree and find the pasted layer's info.
    let tree = engine.layer_tree();
    let mut found_bounds: Option<CanvasRect> = None;
    for info in &tree {
        if let LayerInfo::Raster { id, bounds, .. } = info {
            if *id as u64 == pasted_id.to_ffi() {
                found_bounds = Some(*bounds);
                break;
            }
        }
    }
    let bounds = found_bounds.expect("pasted layer must appear in layer_tree as Raster");
    assert_eq!(
        bounds,
        CanvasRect::from_xywh(-50, -50, pw, ph),
        "LayerInfo bounds must reflect the actual paste extent"
    );

    // Round-trip the bounds field through serde to confirm the FFI
    // serialization preserves the canvas-space offsets and dimensions.
    let json = serde_json::to_string(&bounds).expect("bounds must serialize");
    let decoded: CanvasRect =
        serde_json::from_str(&json).expect("bounds must deserialize byte-identically");
    assert_eq!(decoded, bounds);
    // Frontend-facing JSON contract: `{ "origin": { "x": .., "y": .. }, "width": .., "height": .. }`.
    let value: serde_json::Value =
        serde_json::from_str(&json).expect("bounds JSON must parse as Value");
    assert_eq!(value["origin"]["x"], -50);
    assert_eq!(value["origin"]["y"], -50);
    assert_eq!(value["width"], pw);
    assert_eq!(value["height"], ph);
}

/// Repeated paste → cancel cycles must not leak GPU textures. Regression
/// for P3: `cancel_floating` on the auto-created paste layer disposes its
/// compositor state in addition to detaching the doc node.
#[test]
fn paste_cancel_cycles_dont_leak_layer_textures() {
    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    let baseline = engine.test_node_texture_count();

    // Use a 4×-canvas paste so each leaked texture would be observable —
    // matches the plan's "paste 4K image" intent at test scale.
    let pw: u32 = cw * 4;
    let ph: u32 = ch * 4;
    let rgba = vec![0xAAu8; (pw * ph * 4) as usize];

    for _ in 0..5 {
        let id = engine.paste_image_floating(pw, ph, &rgba, 0, 0, None);
        assert!(engine.has_layer(id), "paste should create the target layer");
        engine.cancel_floating();
        assert!(!engine.has_layer(id), "cancel should detach the layer");
    }

    let after_cycles = engine.test_node_texture_count();
    assert_eq!(
        after_cycles, baseline,
        "5 paste→cancel cycles should leave layer_textures count unchanged \
         (baseline {baseline}, got {after_cycles})"
    );
}

/// `Engine::remove_layer` must dispose the layer's compositor state so
/// repeated add → remove cycles don't leak textures. The undo entry
/// preserves the doc-side metadata; pixel data is intentionally lost on
/// remove (re-inserting on undo gives back an empty raster).
#[test]
fn add_remove_cycles_dont_leak_layer_textures() {
    let (cw, ch) = (128, 128);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer(None);

    let baseline = engine.test_node_texture_count();

    for _ in 0..5 {
        let id = engine.add_raster_layer(None);
        assert!(engine.has_layer(id));
        engine.remove_layer(id).expect("remove should succeed");
        assert!(!engine.has_layer(id));
    }

    let after_cycles = engine.test_node_texture_count();
    assert_eq!(
        after_cycles, baseline,
        "5 add→remove cycles should leave layer_textures count unchanged \
         (baseline {baseline}, got {after_cycles})"
    );
}

/// Growing a layer that has an active mask must rebuild the mask bind
/// group against the new mask texture; otherwise the next render would
/// trip wgpu validation (stale view inside live bind group).
#[test]
fn mid_stroke_growth_invalidates_mask_bind_group() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);
    engine.render(0.0);

    // Paint past the right edge — triggers grow which must rebuild the
    // mask bind group.
    paint_at(
        &mut engine,
        layer_id,
        cw as f32 + 80.0,
        ch as f32 / 2.0,
        1.0,
        0.0,
        0.0,
    );

    // Render — if the bind group still pointed at the dropped mask
    // texture, wgpu validation would flag it.
    engine.render(0.0);

    let bounds = engine.layer_bounds(layer_id).unwrap();
    assert!(bounds.width > cw, "layer should have grown");
}

// ============================================================================
// Floating undo on offset / paste-extent layers
// ============================================================================

/// Transform-commit with rotation: a 90° rotation moves pixels OUTSIDE the
/// source rect saved at `setup_transform`. The new commit-time path-B save
/// covers the affected rect (post-rotation bounds), so the
/// `commit_rect ⊆ saved_rect` invariant holds and undo restores correctly.
/// Without path B, the new debug_assert would fire here.
#[test]
fn floating_transform_undo_with_rotation() {
    use darkly::gpu::transform::{affine_multiply, affine_rotate, affine_translate};

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);

    // Layer with a horizontal red bar across the top half; rotating a
    // selected 16×16 chunk of it will visibly change pixels in the
    // selected region (the post-rotation content differs from the
    // pre-rotation content), so we can detect a real change after commit.
    let pw: u32 = cw;
    let ph: u32 = ch;
    let mut layer_rgba = vec![0u8; (pw * ph * 4) as usize];
    for y in 0..ph {
        for x in 0..pw {
            let idx = ((y * pw + x) * 4) as usize;
            if y < ph / 2 {
                layer_rgba[idx] = 255; // red top half
            } else {
                layer_rgba[idx + 2] = 255; // blue bottom half
            }
            layer_rgba[idx + 3] = 255;
        }
    }
    let layer_id = engine.paste_image(pw, ph, &layer_rgba, 0, 0, None);

    // Select the central 16×16 region — straddles the red/blue boundary
    // so a rotation visibly changes pixel values.
    let cx = cw / 2;
    let cy = ch / 2;
    let half = 8u32;
    engine.select_rect(
        (cx - half) as f32,
        (cy - half) as f32,
        (2 * half) as f32,
        (2 * half) as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );

    let before = engine.test_readback_layer(layer_id);

    let started = engine.begin_transform(layer_id);
    assert!(started, "begin_transform with selection should succeed");

    // Rotate the floating content 90° about the source-local center (8,8).
    // After rotation the bounds are still 16×16 (90° on a square), so
    // affected_rect == source_rect — the path-B path is exercised, and
    // the un-clear step ensures the cleared source pixels are restored
    // before the affected-rect save captures the pre-render state.
    let theta = std::f32::consts::FRAC_PI_2;
    let matrix = affine_multiply(
        &affine_translate(8.0, 8.0),
        &affine_multiply(&affine_rotate(theta), &affine_translate(-8.0, -8.0)),
    );
    engine.update_floating_matrix(matrix);

    engine.commit_floating();
    engine.render(0.0);

    let after_commit = engine.test_readback_layer(layer_id);
    assert_ne!(
        before, after_commit,
        "transform commit should have modified the layer"
    );

    engine.undo();
    engine.render(0.0);

    let after_undo = engine.test_readback_layer(layer_id);
    assert_eq!(
        before, after_undo,
        "undo of rotation transform must restore byte-identical pixels"
    );
}

/// Regression: a brush stroke that paints past the canvas edge triggers a
/// mid-stroke layer grow. After the grow, the diff_rect at end_stroke can
/// land in the newly-grown area — a region that was just allocated and
/// (correctly) holds zero/transparent pixels as its pre-stroke state. The
/// commit/restore path must accept this as a contained sub-rect of the
/// snapshot. Pre-fix, the snapshot's saved rect was translated to the old
/// layer's footprint within the new layer, so a diff covering newly-grown
/// pixels would (a) panic the new debug_assert, locking the engine RefCell
/// in WASM and (b) read the correct zero-init pixels in release.
#[test]
fn brush_stroke_off_canvas_undo_after_grow() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    let before = engine.test_readback_layer(layer_id);

    // Paint well past the right edge — forces a grow, then the dab
    // lands in the newly-grown region.
    paint_at(
        &mut engine,
        layer_id,
        cw as f32 + 80.0,
        ch as f32 / 2.0,
        1.0,
        0.0,
        0.0,
    );

    let after_paint = engine.test_readback_layer(layer_id);
    assert_ne!(
        before.len(),
        after_paint.len(),
        "stroke past edge should have grown the layer texture"
    );

    // Undo: must succeed without panic, and the layer should match its
    // pre-stroke state where it overlaps the original bounds. (The grown
    // texture is larger; we only assert that the undo didn't crash and
    // that pixels in the original region are restored to transparent —
    // there was no pre-stroke layer content past `before.len()`.)
    engine.undo();
    engine.render(0.0);

    let after_undo = engine.test_readback_layer(layer_id);
    // The original-bounds region must be transparent (= pre-stroke state).
    let n = (cw * ch * 4) as usize;
    let original_region_post_undo = &after_undo[..n.min(after_undo.len())];
    let any_opaque = original_region_post_undo
        .chunks_exact(4)
        .any(|px| px[3] > 0);
    assert!(
        !any_opaque,
        "after undo, original-bounds region should be fully transparent"
    );
}

/// Regression: a multi-dab stroke that crosses the canvas edge mid-stroke
/// must keep its EARLY (pre-grow) dabs at their original canvas positions.
/// Pre-fix, the brush engine's per-dab `save_points` and the
/// `checkpoint_ring` cached layer-local bboxes that became stale after
/// `grow_layer_texture` shifted the layer's local origin. On the next
/// stroke event, `restore_before` blitted the checkpoint back at the
/// stale (old-frame) layer-local position — corresponding to a canvas
/// position offset by `(dx, dy)` toward the growth direction. Visible
/// symptom: the entire stroke shifted outward toward the chunk being
/// added.
#[test]
fn stroke_crossing_canvas_edge_keeps_early_dabs_in_place() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Stroke from canvas (50, 100) to (-100, 100). The dab center crosses
    // x=0 partway through, triggering a negative-direction grow that
    // shifts `offset_x` to ≤ -256.
    engine.begin_stroke(layer_id);
    for step in 0..20 {
        let t = step as f32 / 19.0;
        let x = 50.0 - t * 150.0;
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: 100.0,
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
    engine.render(0.0);

    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    assert!(
        bounds.origin.x <= -256,
        "negative-direction grow should have shifted offset_x; got {}",
        bounds.origin.x
    );

    // Read the layer back. It's now the post-grow size. Find the painted
    // pixel for the FIRST dab (canvas (50, 100)) — should appear at
    // layer-local (50 - offset_x, 100 - offset_y).
    let pixels = engine.test_readback_layer(layer_id);
    let lw = bounds.width;
    let early_lx = (50 - bounds.origin.x) as u32;
    let early_ly = (100 - bounds.origin.y) as u32;

    // Search a small box around the expected position.
    let mut hit_at_expected = false;
    for dy in 0..8u32 {
        for dx in 0..8u32 {
            let px = early_lx.saturating_sub(4) + dx;
            let py = early_ly.saturating_sub(4) + dy;
            if alpha_at(&pixels, lw, px, py) > 0 {
                hit_at_expected = true;
                break;
            }
        }
    }
    assert!(
        hit_at_expected,
        "early-stroke dab at canvas (50, 100) must land at layer-local ({early_lx}, {early_ly}) after grow"
    );

    // Sanity: also check that paint did NOT land at the SHIFTED position
    // (where the bug would put it). The bug shifts by (dx, dy) =
    // (offset_x_old - offset_x_new, ...) = (256, 0). So the early dab
    // would erroneously appear at layer-local (50, 100) (no offset).
    let mut wrong_hit = 0u8;
    for dy in 0..8u32 {
        for dx in 0..8u32 {
            let px = (50u32).saturating_sub(4) + dx;
            let py = (100u32).saturating_sub(4) + dy;
            wrong_hit = wrong_hit.max(alpha_at(&pixels, lw, px, py));
        }
    }
    assert_eq!(
        wrong_hit, 0,
        "no paint should land at the un-translated (50, 100) position; that area is canvas (50 + offset_x, 100) and should be empty"
    );
}

/// Regression: after stroke A (inside canvas) and stroke B (off-canvas,
/// triggers grow), undoing both must leave a clean layer. Pre-fix, the
/// pending diff for stroke A was computed in stroke A's frame, but its
/// commit ran AFTER stroke B's grow rebased the scratch — so the saved
/// undo buffer held wrong pixels and `restore_region` wrote them at the
/// stale layer-local coords, missing where stroke A actually landed in
/// the post-grow layer. Symptom: stroke A's pixels persist after both
/// undos.
#[test]
fn undo_after_grow_does_not_leave_prior_stroke_artifacts() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Stroke A: canvas (50, 50), inside the 256×256 canvas. No grow.
    paint_at(&mut engine, layer_id, 50.0, 50.0, 1.0, 0.0, 0.0);

    // Stroke B: canvas (-100, -100), triggers a negative-direction grow.
    // This is the event that processes stroke A's pending diff against
    // the post-grow scratch, corrupting its undo entry.
    paint_at(&mut engine, layer_id, -100.0, -100.0, 0.0, 1.0, 0.0);

    // Undo B, then A.
    engine.undo();
    engine.render(0.0);
    engine.undo();
    engine.render(0.0);

    // Layer should be fully transparent — both strokes undone.
    let pixels = engine.test_readback_layer(layer_id);
    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    let (lw, lh) = (bounds.width, bounds.height);

    let mut painted_count = 0u32;
    for y in 0..lh {
        for x in 0..lw {
            if alpha_at(&pixels, lw, x, y) > 0 {
                painted_count += 1;
            }
        }
    }
    assert_eq!(
        painted_count, 0,
        "after undoing both strokes, layer should be fully transparent; \
         got {painted_count} painted pixels (artifacts from pre-grow stroke)"
    );
}

/// Regression: translating a transform without a selection must not leave
/// a duplicate copy of the source at the original position. The
/// `commit_floating` un-clear restores source pixels to the layer at the
/// source rect (so the undo-buffer save captures the pre-transform state)
/// — but the transform render shader uses `discard` outside transformed
/// bounds, so without a re-clear the un-cleared source pixels remain on
/// the layer alongside the transformed source.
#[test]
fn transform_translate_no_selection_does_not_duplicate() {
    use darkly::gpu::transform::affine_translate;

    let (cw, ch) = (128u32, 128u32);
    let mut engine = test_engine(cw, ch);

    // Paste a canvas-sized image with a 16×16 red square at canvas (10, 10)
    // and the rest transparent. Layer bounds = full canvas, so the
    // translated transform position is inside the layer texture.
    let mut rgba = vec![0u8; (cw * ch * 4) as usize];
    for y in 10..26 {
        for x in 10..26 {
            let idx = ((y * cw + x) * 4) as usize;
            rgba[idx] = 255; // R
            rgba[idx + 3] = 255; // A
        }
    }
    let layer_id = engine.paste_image(cw, ch, &rgba, 0, 0, None);

    // No selection — drives the async content_bounds compute path.
    let started = engine.begin_transform(layer_id);
    if !started {
        for _ in 0..16 {
            engine.test_flush_readbacks();
            engine.render(0.0);
            if engine.has_floating() {
                break;
            }
        }
    }
    assert!(
        engine.has_floating(),
        "begin_transform should have set up floating"
    );

    // Translate by (50, 50): source content at canvas (10, 10) → (60, 60).
    engine.update_floating_matrix(affine_translate(50.0, 50.0));
    engine.commit_floating();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer_id);
    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    let lw = bounds.width;
    let ox = bounds.origin.x;
    let oy = bounds.origin.y;

    let alpha_canvas = |cx: i32, cy: i32| -> u8 {
        let lx = cx - ox;
        let ly = cy - oy;
        if lx < 0 || ly < 0 || lx as u32 >= bounds.width || ly as u32 >= bounds.height {
            return 0;
        }
        alpha_at(&pixels, lw, lx as u32, ly as u32)
    };

    // Translated position: alpha must be present.
    assert!(
        alpha_canvas(65, 65) > 0,
        "translated source position (65, 65) must be opaque after commit; got A={}",
        alpha_canvas(65, 65)
    );

    // Original source position: alpha must be zero. Pre-fix this would
    // still hold the un-cleared source pixel, producing a duplicate.
    assert_eq!(
        alpha_canvas(15, 15),
        0,
        "original source position (15, 15) must be transparent after \
         commit — non-zero here means the un-clear left a duplicate of \
         the source at its original position"
    );
}

/// Regression: same as the no-selection version, but with an active
/// selection covering the source square. The selection branch of
/// `setup_transform` does a selection-shaped clear (`erase_with_selection`)
/// rather than a full-rect clear; commit must replay that same shape so the
/// transform shader's `discard`-outside-transformed-bounds doesn't leave
/// the un-cleared source pixels at the original position.
#[test]
fn transform_translate_with_selection_does_not_duplicate() {
    use darkly::gpu::transform::affine_translate;

    let (cw, ch) = (128u32, 128u32);
    let mut engine = test_engine(cw, ch);

    // Same canvas-sized image as the no-selection test: a 16×16 red square
    // at canvas (10, 10).
    let mut rgba = vec![0u8; (cw * ch * 4) as usize];
    for y in 10..26 {
        for x in 10..26 {
            let idx = ((y * cw + x) * 4) as usize;
            rgba[idx] = 255; // R
            rgba[idx + 3] = 255; // A
        }
    }
    let layer_id = engine.paste_image(cw, ch, &rgba, 0, 0, None);

    // Select exactly the red square. select_rect is synchronous and
    // populates gpu_selection.cpu_cache eagerly via upload_replace, so
    // begin_transform takes the synchronous selection branch.
    engine.select_rect(10.0, 10.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);

    let started = engine.begin_transform(layer_id);
    assert!(
        started,
        "begin_transform should set up floating synchronously with an active selection"
    );

    engine.update_floating_matrix(affine_translate(50.0, 50.0));
    engine.commit_floating();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer_id);
    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    let lw = bounds.width;
    let ox = bounds.origin.x;
    let oy = bounds.origin.y;

    let alpha_canvas = |cx: i32, cy: i32| -> u8 {
        let lx = cx - ox;
        let ly = cy - oy;
        if lx < 0 || ly < 0 || lx as u32 >= bounds.width || ly as u32 >= bounds.height {
            return 0;
        }
        alpha_at(&pixels, lw, lx as u32, ly as u32)
    };

    assert!(
        alpha_canvas(65, 65) > 0,
        "translated source position (65, 65) must be opaque after commit; got A={}",
        alpha_canvas(65, 65)
    );

    assert_eq!(
        alpha_canvas(15, 15),
        0,
        "original source position (15, 15) must be transparent after commit — \
         non-zero here means the selection-shaped re-clear was skipped and the \
         un-cleared source pixel was preserved by the transform shader's discard"
    );
}

/// Regression for canvas-coord storage of pending undo commits: a deferred
/// `pending_undo_commit` from stroke A must remain valid when stroke B grows
/// the layer a second time before A's diff has been polled. With layer-local
/// coords the diff rect captured at A's request time would be invalidated by
/// B's grow rebasing the scratch and shifting the local frame, so the diff
/// would land at the wrong texels. Canvas coords are stable across grows, so
/// this round-trips cleanly.
#[test]
fn pending_undo_commit_survives_two_grows() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Stroke A: off-canvas in -X direction. Triggers grow #1.
    paint_at(&mut engine, layer_id, -50.0, 50.0, 1.0, 0.0, 0.0);
    // Stroke B: off-canvas in -Y direction. Triggers grow #2 before A's
    // diff has been polled (the deferred commit holds the canvas-coord
    // snapshot from before grow #1).
    paint_at(&mut engine, layer_id, 50.0, -50.0, 0.0, 1.0, 0.0);

    // Undo both strokes. After both undos the layer must be fully
    // transparent — if A's deferred commit captured the wrong pixels,
    // some red would remain visible.
    engine.undo();
    engine.render(0.0);
    engine.undo();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer_id);
    let bounds = engine.layer_bounds(layer_id).expect("layer exists");
    let (lw, lh) = (bounds.width, bounds.height);

    let mut painted_count = 0u32;
    for y in 0..lh {
        for x in 0..lw {
            if alpha_at(&pixels, lw, x, y) > 0 {
                painted_count += 1;
            }
        }
    }
    assert_eq!(
        painted_count, 0,
        "after undoing two strokes that each grew the layer, the layer \
         should be fully transparent; got {painted_count} painted pixels — \
         the deferred undo commit from stroke A held a stale layer-local \
         rect that survived past the second grow"
    );
}

// ============================================================================
// Mask painting — regression tests for brush-stroke-on-mask
//
// Defends against silent failure when painting onto R8 mask textures
// (the brush stack must not assume an RGBA8 destination).
// ============================================================================

/// Paint a single black brush dab at (x, y) on a mask. Brush color is
/// grayscale (R=G=B=0); the R channel is what lands in the R8 mask.
fn paint_mask_dab(engine: &mut DarklyEngine, host_id: LayerId, x: f32, y: f32, value: f32) {
    // The new model paints on the mask modifier id directly, not via a
    // session redirect from the host. Resolve the mask id and stroke on it.
    let mask_id = engine
        .host_mask_id(host_id)
        .expect("paint_mask_dab requires the host to have a mask modifier");
    engine.begin_stroke(mask_id);
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

/// Sample the R channel from an R8 (one byte per pixel) mask buffer.
fn mask_byte_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[(y * w + x) as usize]
}

/// Brush stroke onto a layer mask must update the mask texture.
///
/// Pre-fix (with brush pipeline hardcoded to RGBA8) this fails: the
/// commit-side format mismatch means painting silently no-ops, and the
/// mask remains all-white at value 255.
#[test]
fn engine_brush_stroke_paints_on_mask() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);

    paint_mask_dab(&mut engine, layer_id, (w / 2) as f32, (h / 2) as f32, 0.0);

    let pixels = engine.test_readback_mask(layer_id);
    assert_eq!(
        pixels.len(),
        (w * h) as usize,
        "mask is R8 — one byte/pixel"
    );
    let center = mask_byte_at(&pixels, w, w / 2, h / 2);
    assert!(
        center < 250,
        "mask center should be painted (byte < 250 after a black brush dab); \
         got {center} — brush stroke did not modify the mask"
    );
}

/// Pixels untouched by the brush dab must remain at their pre-stroke value
/// byte-exactly. Validates that the format-aware commit + R8→RGBA8 read
/// blit round-trip preserves bytes for unmodified regions.
#[test]
fn engine_mask_brush_unstroked_pixels_unchanged() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);

    paint_mask_dab(&mut engine, layer_id, 10.0, 10.0, 0.0);

    let pixels = engine.test_readback_mask(layer_id);
    let far = mask_byte_at(&pixels, w, 100, 100);
    assert_eq!(
        far, 255,
        "pixel at (100,100) — well outside the dab footprint at (10,10) — \
         must remain at the initial reveal-all value (255); got {far} — \
         the read-side R8→RGBA8 expand or write-side RGBA8→R8 reduce \
         shifted bytes"
    );
}

/// Undo of a mask brush stroke must restore the mask to its pre-stroke
/// (all-white) state.
#[test]
fn engine_mask_brush_undo_restores_mask() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);

    paint_mask_dab(&mut engine, layer_id, (w / 2) as f32, (h / 2) as f32, 0.0);
    // Brush-stroke commit is async (diff-rect compute). Flush so the
    // GpuRegionAction is on the undo stack before we call `undo()`.
    engine.test_flush_readbacks();
    engine.render(0.0);
    engine.undo();
    engine.render(0.0);

    let pixels = engine.test_readback_mask(layer_id);
    let mut all_white = true;
    for byte in &pixels {
        if *byte != 255 {
            all_white = false;
            break;
        }
    }
    assert!(
        all_white,
        "after undo of mask brush stroke, mask should return to all-white"
    );
}

/// Brush stroke onto a mask must respect an active selection: pixels
/// inside the selection get painted, pixels outside are preserved.
#[test]
fn engine_mask_brush_respects_selection() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);

    // add_mask ran with no selection, so the mask starts all-white (255);
    // selection-seeding is bypassed. Then select the left half.
    engine.select_rect(
        0.0,
        0.0,
        (w / 2) as f32,
        h as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );

    paint_mask_dab(&mut engine, layer_id, (w / 4) as f32, (h / 2) as f32, 0.0);
    paint_mask_dab(
        &mut engine,
        layer_id,
        (3 * w / 4) as f32,
        (h / 2) as f32,
        0.0,
    );

    let pixels = engine.test_readback_mask(layer_id);
    let inside = mask_byte_at(&pixels, w, w / 4, h / 2);
    let outside = mask_byte_at(&pixels, w, 3 * w / 4, h / 2);
    assert!(
        inside < 250,
        "mask byte inside the selection should be painted (< 250); got {inside}"
    );
    assert_eq!(
        outside, 255,
        "mask byte outside the selection must remain all-reveal (255) — \
         brush stroke on a mask must respect the active selection; got {outside}"
    );
}

/// Adding a mask while a selection is active seeds the new mask from
/// the selection. This gives users a one-click "selection → mask"
/// gesture: pixels inside the selection reveal (255), pixels outside
/// hide (0).
#[test]
fn engine_add_mask_seeds_from_active_selection() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    engine.select_rect(
        0.0,
        0.0,
        (w / 2) as f32,
        h as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );

    engine.add_mask(layer_id);

    let pixels = engine.test_readback_mask(layer_id);
    let inside = mask_byte_at(&pixels, w, w / 4, h / 2);
    let outside = mask_byte_at(&pixels, w, 3 * w / 4, h / 2);
    assert!(
        inside > 200,
        "mask byte inside the selection should reveal (~255); got {inside}"
    );
    assert!(
        outside < 50,
        "mask byte outside the selection should hide (~0); got {outside}"
    );
}

/// Adding a mask without an active selection produces an all-reveal
/// mask (255 everywhere) — the selection-seeding path must not affect
/// the no-selection case.
#[test]
fn engine_add_mask_without_selection_is_all_white() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    engine.add_mask(layer_id);

    let pixels = engine.test_readback_mask(layer_id);
    assert!(
        pixels.iter().all(|&b| b == 255),
        "with no active selection, a freshly-added mask must be all-white (255)"
    );
}

/// In the new modifier-node model, paint targets are addressed by node id.
/// Painting on a host id with no mask attached just paints on the host —
/// there is no separate "edit mask" redirect that could go wrong. This
/// regression test now verifies safety: `begin_stroke` on a host with no
/// mask, plus a stroke, doesn't panic.
#[test]
fn engine_no_mask_brush_safe_on_layer() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // No mask added; stroking on the layer must just paint the layer.
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x: (w / 2) as f32,
        y: (h / 2) as f32,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 0.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

/// FloodFill on a mask paints every pixel reachable from the seed. The
/// `GpuPaintTarget` flood-fill path is already format-aware via
/// `composite_pipeline(self.format)`, so this test should pass even
/// pre-fix; it locks the behavior down so a future refactor can't break
/// it without warning.
#[test]
fn engine_mask_flood_fill() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);
    let mask_id = engine.host_mask_id(layer_id).unwrap();

    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: (w / 2) as f32,
        y: (h / 2) as f32,
        r: 0,
        g: 0,
        b: 0,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
    engine.render(0.0);

    let pixels = engine.test_readback_mask(layer_id);
    let center = mask_byte_at(&pixels, w, w / 2, h / 2);
    assert!(
        center < 10,
        "flood fill with black should drive mask center near 0; got {center}"
    );
}

/// Regression: magic wand with mask editing active must read from the mask
/// (R8) texture, not the layer (RGBA8) texture. Pre-fix it always read the
/// layer — on a freshly-added raster layer the layer is fully transparent,
/// so flood-fill from any seed produced a full-canvas selection regardless
/// of what was painted on the mask.
#[test]
fn engine_magic_wand_on_mask_reads_mask_not_layer() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Seed the mask: select the left half, then add_mask copies the
    // selection into the new mask (left = 255, right = 0).
    engine.select_rect(
        0.0,
        0.0,
        (w / 2) as f32,
        h as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    engine.add_mask(layer_id);
    let mask_id = engine.host_mask_id(layer_id).unwrap();

    // Magic wand seeded inside the left (revealed) half with tolerance 0.
    // On the mask this picks up only the connected 255 region (left half).
    // Pre-fix the wand would read from the layer (transparent everywhere)
    // and select the full canvas regardless of mask state — fixed by
    // dispatching format from the active node id.
    engine.select_magic_wand(
        mask_id,
        darkly::coord::CanvasPoint::new(4, (h / 2) as i32),
        0,
        SelectionMode::Replace,
    );
    engine.test_flush_readbacks();

    let cache = engine
        .test_selection_cpu_cache()
        .expect("magic wand must populate the selection cpu cache");
    let inside = cache[((h / 2) * w + 4) as usize];
    let outside = cache[((h / 2) * w + (3 * w / 4)) as usize];
    assert!(
        inside > 200,
        "seed inside left (mask=255) half must be selected; got {inside}"
    );
    assert_eq!(
        outside, 0,
        "right (mask=0) half must NOT be selected — pre-fix the magic wand \
         flood-filled the empty RGBA layer instead of the mask, producing a \
         full-canvas selection; got {outside}"
    );
}

/// Regression: the flood-fill primitive shared by magic wand and the paint-
/// bucket tool must translate the click coordinate from canvas space into the
/// layer texture's own coordinate frame, and project the resulting mask back
/// into canvas space. Pre-fix both code paths hardcoded the readback rect to
/// `[0, 0, canvas_w, canvas_h]` and treated the seed plus the resulting fill
/// mask as if they were canvas-aligned — but the layer texture can sit at a
/// non-zero canvas offset (paste-extent layers, or layers grown leftward /
/// upward by `ensure_layer_covers_dab`). The result: the seed sampled the
/// wrong texture pixel, the produced mask was layer-local but applied as
/// canvas-aligned, and the selection (or paint deposit) landed shifted from
/// where the user clicked.
#[test]
fn engine_magic_wand_on_paste_extent_layer_translates_coords() {
    use darkly::coord::CanvasRect;

    let (cw, ch) = (64u32, 64u32);
    let mut engine = test_engine(cw, ch);

    // Paste a 96×96 image at canvas (-32, -32). The texture covers canvas
    // (-32, -32) to (64, 64), so the visible canvas region (0..64, 0..64)
    // lives in the texture's bottom-right.
    //
    // Image content: transparent everywhere except an opaque-red 32×32 block
    // at texture (48..80, 48..80) — which projects onto canvas (16..48, 16..48).
    let pw: u32 = 96;
    let ph: u32 = 96;
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    for ty in 48..80u32 {
        for tx in 48..80u32 {
            let i = ((ty * pw + tx) * 4) as usize;
            rgba[i] = 255;
            rgba[i + 3] = 255;
        }
    }
    let pasted = engine.paste_image(pw, ph, &rgba, -32, -32, None);

    assert_eq!(
        engine.layer_bounds(pasted),
        Some(CanvasRect::from_xywh(-32, -32, pw, ph)),
        "paste layer must sit at canvas offset (-32, -32) with the full 96×96 extent"
    );

    // Magic wand seeded at canvas (32, 32) — the visible center of the red
    // block. Tolerance 0 ⇒ flood fill picks up only the connected red pixels.
    engine.select_magic_wand(
        pasted,
        darkly::coord::CanvasPoint::new(32, 32),
        0,
        SelectionMode::Replace,
    );
    engine.test_flush_readbacks();

    let cache = engine
        .test_selection_cpu_cache()
        .expect("magic wand must populate the selection cpu cache");

    // Center of the visible red block: must be selected.
    let inside = cache[(32u32 * cw + 32u32) as usize];
    assert!(
        inside > 200,
        "seed pixel inside the visible red block must be selected; got {inside}"
    );

    // Canvas (0, 0): visible canvas is transparent here, outside the red
    // region ⇒ must NOT be selected. Pre-fix the seed at canvas (32, 32)
    // landed inside an off-canvas transparent region of the texture (because
    // the readback was canvas-rect-anchored rather than texture-anchored), and
    // the resulting full-canvas flood mask got applied with its origin
    // sheared into this corner.
    let outside_tl = cache[0];
    assert_eq!(
        outside_tl, 0,
        "transparent top-left canvas pixel must NOT be selected; got {outside_tl}. \
         Pre-fix the readback rect was hardcoded to the canvas extent, so the seed \
         sampled the wrong texture pixel and the resulting mask got shifted onto \
         the wrong canvas region."
    );

    // Canvas (16, 16): on the boundary of the visible red block — should be
    // selected (top-left corner of the red region).
    let red_corner = cache[(16u32 * cw + 16u32) as usize];
    assert!(
        red_corner > 200,
        "top-left corner of the visible red block (canvas 16,16) must be selected; \
         got {red_corner}"
    );

    // Canvas (48, 48): just outside the red block on the bottom-right ⇒ must
    // NOT be selected.
    let outside_br = cache[(48u32 * cw + 48u32) as usize];
    assert_eq!(
        outside_br, 0,
        "canvas pixel just past the bottom-right edge of the red block must NOT \
         be selected; got {outside_br}"
    );
}

/// Regression: same coordinate bug as `engine_magic_wand_on_paste_extent_layer_translates_coords`,
/// for the paint-bucket / `StrokeOp::FloodFill` path. Pre-fix the seed sampled
/// the wrong texture pixel and the deposited fill color landed shifted from
/// where the user clicked.
#[test]
fn engine_flood_fill_on_paste_extent_layer_translates_coords() {
    let (cw, ch) = (64u32, 64u32);
    let mut engine = test_engine(cw, ch);

    // Same paste-extent layer as the magic-wand regression: 96×96 image at
    // canvas (-32, -32), with an opaque-red 32×32 block at texture
    // (48..80, 48..80) ⇒ visible at canvas (16..48, 16..48).
    let pw: u32 = 96;
    let ph: u32 = 96;
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    for ty in 48..80u32 {
        for tx in 48..80u32 {
            let i = ((ty * pw + tx) * 4) as usize;
            rgba[i] = 255;
            rgba[i + 3] = 255;
        }
    }
    let pasted = engine.paste_image(pw, ph, &rgba, -32, -32, None);

    // Bucket-fill at canvas (32, 32) — center of the visible red block.
    // Tolerance 0 ⇒ only the contiguous red region should change.
    engine.begin_stroke(pasted);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 32.0,
        y: 32.0,
        r: 0,
        g: 255,
        b: 0,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
    engine.render(0.0);

    // Read the raw layer texture (96×96, in layer-local coords) and assert
    // the fill landed in the right place.
    let pixels = engine.test_readback_layer(pasted);

    // Center of the (formerly red) block, texture (64, 64) = canvas (32, 32):
    // must now be opaque green.
    let center_idx = ((64u32 * pw + 64u32) * 4) as usize;
    assert!(
        pixels[center_idx] < 50 && pixels[center_idx + 1] > 200 && pixels[center_idx + 3] > 200,
        "center of the red block (canvas 32,32 = texture 64,64) must be replaced \
         with opaque green; got rgba=({}, {}, {}, {})",
        pixels[center_idx],
        pixels[center_idx + 1],
        pixels[center_idx + 2],
        pixels[center_idx + 3]
    );

    // Texture (40, 40) = canvas (8, 8). On-canvas, outside the red block, so
    // it started transparent. Pre-fix the upload mask was canvas-rect-aliased
    // — the seed read a transparent pixel from the wrong place, the resulting
    // mask covered "everything except the visible red corner of the readback
    // buffer", and the fill_rect deposited green here. Post-fix the mask is
    // properly layer-translated and this pixel stays transparent.
    let stray_idx = ((40u32 * pw + 40u32) * 4) as usize;
    assert_eq!(
        pixels[stray_idx + 3],
        0,
        "texture (40,40) = canvas (8,8) was transparent and disconnected from the \
         red block; it must remain transparent. Pre-fix the bucket fill leaked \
         green here because the readback rect was canvas-anchored rather than \
         texture-anchored. Got rgba=({}, {}, {}, {})",
        pixels[stray_idx],
        pixels[stray_idx + 1],
        pixels[stray_idx + 2],
        pixels[stray_idx + 3]
    );

    // Texture (0, 0) = canvas (-32, -32). Fully off-canvas — must remain
    // transparent regardless of fix (the fill_rect is canvas-clipped).
    assert_eq!(
        pixels[3], 0,
        "off-canvas texture corner must stay transparent; got alpha={}",
        pixels[3]
    );
}

/// Regression: the interactive transform preview must apply the target
/// layer's mask. Pre-fix the transform-blend shader sampled the floating
/// source unconditionally and never sampled the mask, so masked-off regions
/// of the layer "lit back up" as soon as the user began a transform — even
/// though the committed pixels would re-mask on the next blend pass. This
/// produced a flicker-on-grab visual bug.
#[test]
fn floating_preview_respects_layer_mask() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Paint a horizontal red stroke across the full canvas width.
    paint_full_stroke(&mut engine, layer_id, w, h);
    engine.render(0.0);

    // Select the left half, then add a mask. With an active selection,
    // `add_mask` seeds from the selection: left = 255 (reveal), right = 0
    // (hide).
    engine.select_rect(
        0.0,
        0.0,
        (w / 2) as f32,
        h as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    engine.add_mask(layer_id);
    engine.clear_selection();
    engine.render(0.0);

    // Sanity: before the transform begins, the regular blend pass already
    // hides the right half. If this fails the test setup is wrong, not the
    // transform-preview code.
    let pre = engine.test_readback_canvas();
    let pre_left = alpha_at(&pre, w, w / 4, h / 2);
    let pre_right = alpha_at(&pre, w, 3 * w / 4, h / 2);
    assert!(
        pre_left > 0,
        "test setup: left half should be revealed (mask=255); got alpha={pre_left}"
    );
    assert_eq!(
        pre_right, 0,
        "test setup: right half should be hidden (mask=0); got alpha={pre_right}"
    );

    // Begin a transform with no active selection — content bounds are
    // resolved asynchronously via the compositor's GPU compute, so spin
    // a few frames until floating content is live.
    engine.begin_transform(layer_id);
    let mut floating_ready = false;
    for _ in 0..16 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if engine.has_floating() {
            floating_ready = true;
            break;
        }
    }
    assert!(
        floating_ready,
        "begin_transform did not produce floating content within 16 frames"
    );
    // Render once more so the floating preview pass runs on the current frame.
    engine.render(0.0);

    // The transform starts at identity, so the floating preview shows the
    // extracted content at the same canvas position the layer occupied.
    // The mask must still hide the right half, exactly as the regular
    // blend pass did before the transform began.
    let post = engine.test_readback_canvas();
    let post_left = alpha_at(&post, w, w / 4, h / 2);
    let post_right = alpha_at(&post, w, 3 * w / 4, h / 2);
    assert!(
        post_left > 0,
        "left half should still be visible during transform preview; got alpha={post_left}"
    );
    assert_eq!(
        post_right, 0,
        "right half is masked out — the floating preview must apply the \
         target layer's mask. Pre-fix the transform-blend shader skipped \
         the mask entirely, so this read came back fully opaque; got \
         alpha={post_right}"
    );
}

// ============================================================================
// Mask → Modifier-Node regression tests
//
// These tests defend the structural invariants of the modifier-node model:
// the document model has no `has_mask` / `mask_enabled` / `show_mask`
// booleans, masks are real nodes with their own `PixelBuffer`, lockstep
// growth is a document-side operation, and the type system forbids ever
// putting a `Modifier` into the regular tree.
// ============================================================================

/// Painting past the host's bounds grows the host texture and — because the
/// mask is unlocked — grows the mask in lockstep so `host.bounds == mask.bounds`
/// after the stroke. Defends the per-buffer-bounds invariant from §3 of the
/// plan: the blend shader samples each `PixelBuffer` by its own bounds, and
/// lockstep growth keeps the mask UV-coincident with the host without a
/// special "shared UV" case.
#[test]
fn mask_grows_in_lockstep_with_host_when_unlocked() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);
    let mask_id = engine
        .host_mask_id(layer_id)
        .expect("just-added mask must be reachable via host_mask_id");

    let host_before = engine
        .layer_bounds(layer_id)
        .expect("raster layer has bounds");
    let mask_before = engine
        .node_pixel_bounds(mask_id)
        .expect("mask modifier has bounds");
    assert_eq!(
        host_before, mask_before,
        "fresh mask must inherit the host's bounds"
    );

    // Paint past the right edge to force a chunk-aligned growth of the host.
    paint_at(
        &mut engine,
        layer_id,
        cw as f32 + 50.0,
        ch as f32 / 2.0,
        1.0,
        0.0,
        0.0,
    );

    let host_after = engine.layer_bounds(layer_id).expect("layer still exists");
    let mask_after = engine
        .node_pixel_bounds(mask_id)
        .expect("mask still attached");

    assert!(
        host_after.width > host_before.width,
        "host should have grown past canvas; before {} after {}",
        host_before.width,
        host_after.width,
    );
    assert_eq!(
        host_after, mask_after,
        "unlocked mask must follow the host's growth in lockstep — the doc \
         operation in `grow_layer_to_extent` walks `host.modifiers` and \
         resizes each non-locked one. host={host_after:?} mask={mask_after:?}"
    );

    // The newly-grown mask region must be unmasked (255) — the GPU resize
    // clears new pixels to white per `resize_node_texture`, so the host's
    // newly-grown extent samples through cleanly.
    let mask_pixels = engine.test_readback_mask(layer_id);
    let mask_w = mask_after.width;
    let mask_h = mask_after.height;
    assert_eq!(
        mask_pixels.len(),
        (mask_w * mask_h) as usize,
        "mask byte count must match its grown bounds"
    );
    // Sample a pixel in the newly-grown column (just past the original right
    // edge of the canvas, well clear of the brush dab footprint near
    // (cw + 50, ch / 2)). Convert canvas coord → mask-local via origin.
    let probe_canvas_x = cw as i32 + 1;
    let probe_canvas_y = 4i32;
    let local_x = (probe_canvas_x - mask_after.origin.x) as u32;
    let local_y = (probe_canvas_y - mask_after.origin.y) as u32;
    let probe = mask_pixels[(local_y * mask_w + local_x) as usize];
    assert_eq!(
        probe, 255,
        "newly-grown mask region must default to fully-revealed (255); got {probe}"
    );
}

/// A locked mask must NOT follow the host's growth. After the host grows,
/// the mask's `PixelBuffer.bounds` is unchanged — the blend shader samples
/// each buffer by its own bounds, so a diverged mask renders at its frozen
/// position without any special-case shader code (§4 of the plan).
#[test]
fn locked_mask_does_not_follow_host_growth() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id);
    let mask_id = engine.host_mask_id(layer_id).expect("mask exists");

    let mask_bounds_before = engine.node_pixel_bounds(mask_id).expect("mask has bounds");

    // Lock the mask before growing the host.
    engine.set_node_locked(mask_id, true);

    // Force a host growth.
    paint_at(
        &mut engine,
        layer_id,
        cw as f32 + 50.0,
        ch as f32 / 2.0,
        1.0,
        0.0,
        0.0,
    );

    let host_after = engine.layer_bounds(layer_id).expect("layer still exists");
    let mask_after = engine.node_pixel_bounds(mask_id).expect("mask still here");

    assert!(
        host_after.width > cw,
        "test setup precondition: host should have grown past canvas"
    );
    assert_eq!(
        mask_after, mask_bounds_before,
        "locked mask must keep its original bounds; host={host_after:?} \
         mask before/after={mask_after:?}"
    );
    // The mask's GPU texture must still match its (unchanged) bounds.
    let mask_pixels = engine.test_readback_mask(layer_id);
    assert_eq!(
        mask_pixels.len(),
        (mask_after.width * mask_after.height) as usize,
        "locked mask GPU texture must match the unchanged bounds"
    );
}

/// Add → paint → apply → undo round-trip. After `apply_mask` the host's
/// alpha is multiplied by the mask values and the mask modifier is removed.
/// Undo restores both the alpha and the mask modifier (with its pixels).
/// This is the structural replacement for the deleted `MaskPropertyAction`
/// — generic `ModifierAddAction` / `ModifierRemoveAction` plus the existing
/// region-pixel undo cover the round-trip.
#[test]
fn add_paint_apply_undo_round_trip_preserves_mask() {
    let (cw, ch) = (64u32, 64u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // `paint_full_stroke` paints across the canvas at y = h/2 only, so probe
    // a pixel on the painted line. Use (16, h/2) — well inside the stroke
    // path and inside any reasonable mask dab footprint at the same point.
    let probe_x = 16u32;
    let probe_y = ch / 2;
    paint_full_stroke(&mut engine, layer_id, cw, ch);
    engine.test_flush_readbacks();
    engine.render(0.0);
    let host_before_apply = engine.test_readback_layer(layer_id);
    let red_alpha = alpha_at(&host_before_apply, cw, probe_x, probe_y);
    assert!(
        red_alpha > 200,
        "test setup: red stroke should produce opaque alpha at probe; got {red_alpha}"
    );

    // Add a mask, then paint a black dab on the mask at the probe — the
    // alpha at that point will become near 0 after apply.
    engine.add_mask(layer_id);
    let mask_id = engine.host_mask_id(layer_id).expect("mask just added");
    paint_mask_dab(&mut engine, layer_id, probe_x as f32, probe_y as f32, 0.0);

    let mask_pixels_before_apply = engine.test_readback_mask(layer_id);
    let masked_byte = mask_byte_at(&mask_pixels_before_apply, cw, probe_x, probe_y);
    assert!(
        masked_byte < 200,
        "test setup: black dab should drive mask well below 255 at probe; got {masked_byte}"
    );

    // Apply baked the mask alpha into the host RGBA, then removed the modifier.
    engine.apply_mask(layer_id);
    engine.test_flush_readbacks();
    engine.render(0.0);

    assert!(
        engine.host_mask_id(layer_id).is_none(),
        "after apply_mask the modifier must be detached"
    );
    let host_after_apply = engine.test_readback_layer(layer_id);
    let baked_alpha = alpha_at(&host_after_apply, cw, probe_x, probe_y);
    // Apply multiplies alpha by mask byte; the dropped alpha must be strictly
    // less than the original. Anti-aliased dab won't hit zero at the center.
    assert!(
        baked_alpha < red_alpha,
        "apply must multiply alpha by mask at probe; before={red_alpha} after={baked_alpha}"
    );

    // Undo the apply. It pushes three actions in order:
    //   1. GpuRegionAction for the host's pre-multiply alpha
    //   2. GpuRegionAction for the mask's pixels (saved separately so undo
    //      restores them into the freshly re-created texture after step 3)
    //   3. ModifierRemoveAction for the detach
    // Undo runs in reverse: re-attach modifier → restore mask pixels →
    // restore host alpha.
    for _ in 0..3 {
        engine.undo();
        engine.render(0.0);
    }

    let restored_mask_id = engine
        .host_mask_id(layer_id)
        .expect("undo must restore the mask modifier");
    assert_eq!(
        restored_mask_id, mask_id,
        "restored mask must keep its original id (the same Modifier struct \
         is re-attached)"
    );
    let host_after_undo = engine.test_readback_layer(layer_id);
    let restored_alpha = alpha_at(&host_after_undo, cw, probe_x, probe_y);
    assert_eq!(
        restored_alpha, red_alpha,
        "host alpha at probe must be byte-identically restored after undo"
    );
    let mask_after_undo = engine.test_readback_mask(layer_id);
    let restored_mask_byte = mask_byte_at(&mask_after_undo, cw, probe_x, probe_y);
    assert_eq!(
        restored_mask_byte, masked_byte,
        "mask painted byte at probe must be byte-identically restored after undo"
    );
}

/// A passthrough group with a visible mask must apply the mask to its
/// composited children (this is the snapshot+lerp algorithmic path).
/// Toggling the mask invisible turns the same group back into a plain
/// passthrough — no snapshot+lerp, the children render unmasked. The
/// structural detection lives in the compositor's `compose_children`
/// passthrough branch (§6 of the plan): `g.modifiers.mask().filter(|m| m.common.visible)`.
#[test]
fn passthrough_group_with_visible_mask_applies_via_snapshot_lerp() {
    use darkly::document::MoveTarget;

    let (cw, ch) = (64u32, 64u32);
    let mut engine = test_engine(cw, ch);

    let group_id = engine.add_group(None);
    engine.set_group_passthrough(group_id, true);

    let child_id = engine.add_raster_layer(None);
    engine.move_layer(child_id, MoveTarget::IntoGroupTop(group_id));

    // Paint the child red across the canvas.
    paint_full_stroke(&mut engine, child_id, cw, ch);
    engine.render(0.0);

    // Add a mask on the GROUP, then black-out a dab so the group's mask
    // visibly hides part of the child's contribution.
    engine.add_mask(group_id);
    let group_mask_id = engine.host_mask_id(group_id).expect("group has mask");
    engine.begin_stroke(group_mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 4.0,
        y: 4.0,
        r: 0,
        g: 0,
        b: 0,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
    engine.render(0.0);

    // With the mask visible, the masked-off region of the canvas must be
    // transparent (snapshot+lerp ran).
    let masked = engine.test_readback_canvas();
    let masked_alpha = alpha_at(&masked, cw, cw / 2, ch / 2);
    assert_eq!(
        masked_alpha, 0,
        "passthrough-group mask must hide the child's pixels when visible; \
         got alpha={masked_alpha} — the snapshot+lerp branch did not engage"
    );

    // Hide the mask and re-render: the group falls back to plain passthrough,
    // child pixels reappear.
    engine.set_layer_visible(group_mask_id, false);
    engine.render(0.0);

    let unmasked = engine.test_readback_canvas();
    let unmasked_alpha = alpha_at(&unmasked, cw, cw / 2, ch / 2);
    assert!(
        unmasked_alpha > 200,
        "with mask hidden, plain passthrough must let the child's red pixels \
         show through; got alpha={unmasked_alpha}"
    );
}

/// Changing a passthrough group's blend mode must implicitly switch it to
/// isolated — passthrough ignores the group blend mode, so the user's
/// choice would have no visible effect otherwise. Both fields ride a single
/// undo step so one Ctrl-Z restores the original state.
#[test]
fn set_blend_mode_on_passthrough_group_disables_passthrough() {
    use darkly::engine::types::LayerInfo;

    let mut engine = test_engine(64, 64);
    let group_id = engine.add_group(None);
    engine.set_group_passthrough(group_id, true);

    let group_view = |e: &DarklyEngine| -> (bool, &'static str) {
        for node in e.layer_tree() {
            if let LayerInfo::Group {
                id,
                passthrough,
                blend_mode,
                ..
            } = node
            {
                if id as u64 == group_id.to_ffi() {
                    return (passthrough, blend_mode);
                }
            }
        }
        panic!("group not found in layer tree");
    };

    assert_eq!(group_view(&engine), (true, "normal"));

    engine.set_blend_mode(group_id, "multiply");
    assert_eq!(
        group_view(&engine),
        (false, "multiply"),
        "blend-mode change on a passthrough group must clear passthrough"
    );

    // One undo must restore both fields together.
    engine.undo();
    assert_eq!(
        group_view(&engine),
        (true, "normal"),
        "single undo must restore both passthrough and blend mode"
    );

    // Redo replays the bundled change.
    engine.redo();
    assert_eq!(group_view(&engine), (false, "multiply"));

    // A non-passthrough group keeps its passthrough flag untouched when the
    // blend mode changes — the auto-disable only fires when something has
    // to change.
    engine.set_group_passthrough(group_id, false);
    engine.set_blend_mode(group_id, "screen");
    assert_eq!(group_view(&engine), (false, "screen"));
}

/// Type-system check: the `LayerNode` enum must contain ONLY `Layer` and
/// `Group`. Modifiers are not LayerNodes — they're reachable only through
/// their host's `modifiers` field. An exhaustive match (without a wildcard
/// arm) is the compile-time enforcement: adding `LayerNode::Modifier(...)`
/// to the enum would compile but `match` exhaustiveness here would still
/// accept the new variant. To make the intent firm, we destructure the only
/// two legal variants by reference and trigger a non-exhaustive-match error
/// if a third is introduced (the `#[deny(non_exhaustive_omitted_patterns)]`
/// would catch it; `match` exhaustiveness gives us the same signal at the
/// call site).
#[test]
fn layer_node_tree_admits_only_layer_and_group_variants() {
    use darkly::layer::{Layer, LayerGroup, LayerId, LayerNode, RasterLayer};

    fn must_destructure(node: &LayerNode) {
        // Exhaustive match — adding any new `LayerNode::Modifier(...)` arm
        // (or any other variant) to the enum will cause this to stop
        // compiling. That's the type-system enforcement of the plan's
        // §1 invariant: modifiers are NOT LayerNodes.
        match node {
            LayerNode::Layer(Layer::Raster(_)) => {}
            LayerNode::Layer(Layer::Void(_)) => {}
            LayerNode::Group(_) => {}
        }
    }

    // Construct one of each variant to be sure the destructure compiles
    // against the real types and not an accidentally-generic stub.
    let raster = LayerNode::Layer(Layer::Raster(RasterLayer::new(
        LayerId::from_ffi(1),
        darkly::coord::CanvasRect::from_xywh(0, 0, 1, 1),
        "raster".to_string(),
    )));
    let group = LayerNode::Group(LayerGroup::new(LayerId::from_ffi(2), "group".to_string()));
    must_destructure(&raster);
    must_destructure(&group);
}

// ============================================================================
// Selection unification regression tests
//
// The selection is now a typed `Modifier` attached at `Document.selection`.
// The R8 GPU texture lives in the compositor's selection sub-system; the CPU
// cache and tight bounds live on `SelectionModifier`. Bridge ops collapse to
// `clone_modifier_pixels(src, dst)` which is kind-uniform.
// ============================================================================

/// `selection_to_mask` then `mask_to_selection` must round-trip the selection
/// pixels byte-identically. This exercises the §4a unification: both sides
/// of the bridge go through the single `clone_modifier_pixels` helper, so a
/// selection → mask → selection cycle should land identical bytes.
#[test]
fn selection_to_mask_round_trip_preserves_pixels() {
    use darkly::document::SelectionMode;

    let (cw, ch) = (64u32, 64u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer(None);

    // Make a known selection: rectangle in the top-left quadrant.
    engine.select_rect(4.0, 4.0, 20.0, 16.0, SelectionMode::Replace, false, 0.0);
    engine.test_flush_readbacks();
    engine.render(0.0);

    let original_cache = engine
        .test_selection_cpu_cache()
        .expect("Replace path populates the CPU cache eagerly")
        .to_vec();
    assert!(
        original_cache.iter().any(|&b| b > 0),
        "test setup: selection should contain non-zero pixels"
    );

    // Selection → mask. Adds a mask modifier to the layer and seeds it from
    // the selection via `clone_modifier_pixels`.
    engine.add_mask(layer_id);
    engine.selection_to_mask(layer_id);
    engine.render(0.0);

    let mask_pixels = engine.test_readback_mask(layer_id);
    assert_eq!(
        mask_pixels.len(),
        original_cache.len(),
        "mask + selection must be the same canvas size"
    );
    assert_eq!(
        mask_pixels, original_cache,
        "selection_to_mask must copy bytes through `clone_modifier_pixels` \
         without any transformation"
    );

    // Clear the selection, then mask → selection. The selection should come
    // back byte-identical to what we started with.
    engine.clear_selection();
    engine.test_flush_readbacks();
    engine.render(0.0);
    assert!(
        !engine.has_selection(),
        "clear_selection must deactivate the selection"
    );

    let mask_id = engine.host_mask_id(layer_id).expect("mask still attached");
    engine.mask_to_selection(mask_id);
    engine.test_flush_readbacks();
    engine.render(0.0);

    assert!(
        engine.has_selection(),
        "mask_to_selection must reactivate the selection modifier"
    );
    let restored = engine
        .test_selection_cpu_cache()
        .expect("readback after mask_to_selection populates the CPU cache")
        .to_vec();
    assert_eq!(
        restored, original_cache,
        "round-trip must be byte-identical: clone_modifier_pixels copies one \
         R8 texture into another with no algorithmic change in either direction"
    );
}

/// The document model must expose the selection as a typed [`Modifier`] —
/// not as a parallel `Option<AlphaMask>` slot. `Document.selection` is a
/// Modifier with `kind = Selection(...)`, addressable through the same
/// `Modifier::pixels()` interface as a mask.
#[test]
fn document_selection_is_a_typed_modifier() {
    use darkly::document::ModifierKind;

    let (cw, ch) = (32u32, 32u32);
    let engine = test_engine(cw, ch);

    // `DarklyEngine::new` allocates the selection modifier eagerly (visible
    // = false initially, since no selection is logically active yet).
    let modifier_id = engine
        .selection_modifier_id_test()
        .expect("DarklyEngine::new must eagerly allocate the selection modifier");
    assert_ne!(
        modifier_id.to_ffi(),
        0,
        "selection modifier id must be a real id allocated from the document"
    );

    // Initially inactive (no selection painted yet).
    assert!(
        !engine.has_selection(),
        "fresh engine must report no active selection"
    );

    // The selection is reachable through the same kind-uniform paths a mask is.
    let kind_is_selection = engine
        .test_selection_modifier_kind_is_selection()
        .expect("selection modifier must be present");
    assert!(
        kind_is_selection,
        "Document.selection.kind must be ModifierKind::Selection — the \
         type-system unification of selection and mask under Modifier"
    );

    // Pixel-bearing: same `pixels()` accessor as masks. This proves the
    // structural sharing — a future `clone_modifier_pixels(...)` between any
    // two pixel-bearing modifiers (mask, selection, future filter cache)
    // works through one interface.
    let bounds = engine
        .test_selection_pixel_buffer_bounds()
        .expect("SelectionModifier must hold a PixelBuffer");
    assert_eq!(
        (bounds.width, bounds.height),
        (cw, ch),
        "selection PixelBuffer must cover the full canvas"
    );

    // Suppress the unused warning about ModifierKind imports — the test
    // exercises it through the `_kind_is_selection` helper.
    let _ = std::any::type_name::<ModifierKind>();
}

// ============================================================================
// Layer isolation — Krita/Photoshop "alt+click to solo" feature.
// ============================================================================

/// Helper: read the RGBA at canvas pixel (x, y).
fn rgba_at(pixels: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Paint a flood-fill of straight RGBA `(r, g, b, 255)` across `layer_id`.
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
}

/// Isolating a sibling raster must skip the off-path layer in the compose
/// walk — the canvas shows only the isolated layer's color, regardless of
/// stacking order.
#[test]
fn isolate_skips_off_path_sibling_rasters() {
    let (cw, ch) = (32u32, 32u32);
    let mut engine = test_engine(cw, ch);

    let bottom = engine.add_raster_layer(None);
    fill_layer(&mut engine, bottom, 0, 0, 255); // Blue underneath
    let top = engine.add_raster_layer(None);
    fill_layer(&mut engine, top, 255, 0, 0); // Red on top
    engine.test_flush_readbacks();
    engine.render(0.0);

    // No isolation: top layer wins → red.
    let normal = engine.test_readback_canvas();
    let px = rgba_at(&normal, cw, cw / 2, ch / 2);
    assert!(
        px[0] > 200 && px[2] < 50,
        "without isolation, top red layer should show; got {px:?}"
    );

    // Isolate the bottom layer → top is off-path and skipped, blue shows.
    engine.set_isolated_node(Some(bottom));
    engine.render(0.0);
    let isolated = engine.test_readback_canvas();
    let px = rgba_at(&isolated, cw, cw / 2, ch / 2);
    assert!(
        px[2] > 200 && px[0] < 50,
        "isolating the bottom layer must hide the top; got {px:?}"
    );

    // Clear isolation → top layer reappears.
    engine.set_isolated_node(None);
    engine.render(0.0);
    let restored = engine.test_readback_canvas();
    assert_eq!(
        restored, normal,
        "clearing isolation must produce the same pixels as before isolating"
    );
}

/// Isolation is session-only — toggling it on and off must not perturb any
/// layer's `visible` doc state. Hide a layer manually, isolate a sibling,
/// clear isolation: the manually-hidden layer must still be hidden, with
/// no eye-icon state mutation under the hood.
#[test]
fn isolation_does_not_mutate_layer_visibility() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    let red = engine.add_raster_layer(None);
    fill_layer(&mut engine, red, 255, 0, 0);
    let green = engine.add_raster_layer(None);
    fill_layer(&mut engine, green, 0, 255, 0);
    let blue = engine.add_raster_layer(None);
    fill_layer(&mut engine, blue, 0, 0, 255);

    // User hides the red layer manually. Doc state: red.visible = false.
    engine.set_layer_visible(red, false);
    engine.test_flush_readbacks();
    engine.render(0.0);
    let baseline = engine.test_readback_canvas();
    let px = rgba_at(&baseline, cw, cw / 2, ch / 2);
    assert!(px[2] > 200, "baseline: blue (top) should show; got {px:?}");

    // Isolate green. Render — only green should appear.
    engine.set_isolated_node(Some(green));
    engine.render(0.0);
    let solo = engine.test_readback_canvas();
    let px = rgba_at(&solo, cw, cw / 2, ch / 2);
    assert!(
        px[1] > 200 && px[0] < 50 && px[2] < 50,
        "isolated green should be the only thing rendered; got {px:?}"
    );

    // Clear isolation. The hidden-red state must persist — the canvas must
    // match the pre-isolation baseline byte-for-byte. If isolation had
    // mutated `visible` and restored from a snapshot, there'd be a window
    // for the manual `set_layer_visible(red, false)` to be clobbered or
    // mis-restored. Round-tripping through the toggle is the regression.
    engine.set_isolated_node(None);
    engine.render(0.0);
    let after = engine.test_readback_canvas();
    assert_eq!(
        after, baseline,
        "clearing isolation must round-trip exactly to the pre-isolation \
         render — anything else means visibility was puppetted"
    );
}

/// Regression: the present shader used to do `vec4f(color.rgb, 1.0)`,
/// discarding the (premultiplied) alpha channel. With nothing opaque
/// underneath — the canonical case is an isolated layer, where the root
/// accumulator clears to fully transparent and the off-path subtrees are
/// skipped — that turned partial-alpha pixels into darkened-opaque pixels
/// (a 50% red `[0.5, 0, 0, 0.5]` displayed as dark red `[0.5, 0, 0, 1]`)
/// and fully transparent pixels into solid black. The fix composites the
/// premultiplied source over a screen-space checker in the present shader,
/// so any transparency in the final composite reads as transparency.
///
/// This test isolates an empty raster layer (everything off-path is skipped,
/// the layer itself contributes zero alpha) and asserts the present output
/// is the checker pattern, not solid black.
#[test]
fn isolated_transparency_presents_as_checker_not_black() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    // Off-path opaque content — must not leak through isolation.
    let bg = engine.add_raster_layer(None);
    fill_layer(&mut engine, bg, 255, 0, 0);
    // Empty raster — when isolated, the canvas resolves to fully transparent.
    let empty = engine.add_raster_layer(None);

    engine.set_isolated_node(Some(empty));
    engine.test_flush_readbacks();
    engine.render(0.0);

    let pixels = engine.test_readback_present();

    // Checker is screen-space, 8px tiles, gray values 0.4 (102) and 0.6 (153).
    // With identity view transform, screen pixel (0, 0) lands in cell (0, 0)
    // → parity 0 → gray 0.4 → 102. Pixel (8, 0) → cell (1, 0) → parity 1 →
    // gray 0.6 → 153. Pre-fix, both pixels would be (0, 0, 0, 255).
    let cell_a = rgba_at(&pixels, cw, 0, 0);
    let cell_b = rgba_at(&pixels, cw, 8, 0);
    assert_eq!(
        cell_a,
        [102, 102, 102, 255],
        "isolated-empty canvas pixel (0, 0) must show the darker checker tile"
    );
    assert_eq!(
        cell_b,
        [153, 153, 153, 255],
        "isolated-empty canvas pixel (8, 0) must show the lighter checker tile"
    );
}

/// Regression: the present shader's checker composite first treated the
/// composite cache as premultiplied (`color.rgb + checker * (1 - color.a)`),
/// but `composite.wgsl` divides `out_rgb` by `out_a` so the cache is
/// straight-alpha. The premul formula made every partial-alpha pixel display
/// near full intensity (a 50% red `[1, 0, 0, 0.5]` came out `[1.2, 0.2, 0.2]`
/// → clamped `[1, 0.2, 0.2]`), which read on screen as a hard threshold —
/// fully-painted areas were opaque, unpainted areas were checker, with no
/// soft midtone in between. The fix multiplies `color.rgb` by `color.a`
/// before blending over the checker.
///
/// Reproduces by isolating a 50% opacity opaque layer: the composite cache
/// resolves to `(1, 0, 0, 0.5)` straight-alpha, which must present as red
/// genuinely blended halfway with the checker — not as full red.
#[test]
fn isolated_partial_alpha_blends_with_checker_not_at_full_intensity() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);

    let bg = engine.add_raster_layer(None);
    fill_layer(&mut engine, bg, 0, 255, 0); // off-path opaque green
    let translucent = engine.add_raster_layer(None);
    fill_layer(&mut engine, translucent, 255, 0, 0);
    engine.set_opacity(translucent, 0.5);

    engine.set_isolated_node(Some(translucent));
    engine.test_flush_readbacks();
    engine.render(0.0);

    let pixels = engine.test_readback_present();

    // Cell (0, 0) → checker = 0.4 (102). Straight-alpha source-over with
    // src=(1,0,0,0.5):  out = src.rgb*0.5 + checker*0.5
    //                     R = 0.5  + 0.4*0.5 = 0.7  → 178
    //                     G = 0.0  + 0.4*0.5 = 0.2  → 51
    //                     B = 0.0  + 0.4*0.5 = 0.2  → 51
    let cell_a = rgba_at(&pixels, cw, 0, 0);
    let r_dark = cell_a[0];
    let g_dark = cell_a[1];
    assert!(
        (170..=185).contains(&r_dark),
        "translucent red over the dark checker tile must blend halfway — \
         expected R ~178, got {r_dark} (full {cell_a:?}). The pre-fix \
         premul-formula bug clamps R to 255."
    );
    assert!(
        (45..=60).contains(&g_dark),
        "checker green channel must show through — expected G ~51, got \
         {g_dark} (full {cell_a:?}). Pre-fix the formula clamped G to ~76 \
         and hid the checker entirely."
    );

    // Cell (8, 0) → checker = 0.6 (153). Adjacent cell must visibly differ —
    // a hard-threshold bug yields identical pixels across the whole stroke.
    let cell_b = rgba_at(&pixels, cw, 8, 0);
    assert_ne!(
        cell_a, cell_b,
        "adjacent checker cells must differ under the translucent layer — \
         identical pixels mean the present shader is binarizing alpha."
    );
}

/// Repeated identity transforms on a mask must leave the mask byte-for-byte
/// unchanged. Regression for the `transform_commit.wgsl` R8 branch that
/// computed `dot(rgb, luminance_coeffs)`: an R8 texture sampled into vec4
/// returns `(R, 0, 0, 1)`, so the dot multiplied every committed pixel by
/// 0.2126 — every commit darkened the mask, repeated commits compounded.
/// One identity round-trip is enough to catch the bug; doing five proves
/// idempotency under composition.
#[test]
fn repeated_identity_transforms_on_mask_are_idempotent() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);
    let host = engine.add_raster_layer(None);
    fill_layer(&mut engine, host, 255, 255, 255);
    engine.add_mask(host);
    let mask_id = engine.host_mask_id(host).expect("host has mask");

    // Mid-gray fill on the mask. Cleanly tests the multiplicative bug:
    // each darkening pass would push 128 → 27 → 6 → ~1.
    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 128,
        g: 128,
        b: 128,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();

    let baseline = engine.test_readback_mask(host);
    assert!(
        baseline.iter().all(|&v| v == 128),
        "fixture: mask should be mid-gray everywhere before transforming"
    );

    for cycle in 0..5 {
        // Selection-driven transform path: synchronous, no async bounds.
        // The selection-based path also exercises `erase_with_selection`
        // for the source clear, which masks specifically used to break.
        engine.select_all();
        let started = engine.begin_transform(mask_id);
        assert!(
            started,
            "begin_transform on mask must succeed (cycle {cycle})"
        );
        // Identity matrix — commit must be a no-op semantically.
        engine.commit_floating();

        let after = engine.test_readback_mask(host);
        assert_eq!(
            after,
            baseline,
            "identity transform on mask must preserve every pixel \
             (cycle {cycle}); first diff at index {:?}",
            after.iter().zip(&baseline).position(|(a, b)| a != b)
        );
    }
}

/// Isolating a mask modifier renders the host's mask channel as grayscale
/// on the canvas, regardless of the host's color. This is the "show mask"
/// workflow: the mask becomes the canvas. Skipping siblings is the same
/// path the raster case uses; here we additionally verify the host's
/// `isolated` blend uniform engages so the shader picks the grayscale
/// path.
#[test]
fn isolating_mask_modifier_renders_grayscale() {
    let (cw, ch) = (32u32, 32u32);
    let mut engine = test_engine(cw, ch);

    let layer = engine.add_raster_layer(None);
    fill_layer(&mut engine, layer, 255, 0, 0); // Red host.

    engine.add_mask(layer);
    let mask_id = engine.host_mask_id(layer).expect("layer has a mask");

    // Fill the mask with mid-gray (~50% coverage). With a normal render
    // the canvas would show red at half opacity over transparent.
    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 128,
        g: 128,
        b: 128,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();

    // Isolate the mask modifier itself — host renders as grayscale of its
    // mask channel, fully opaque. No red anywhere.
    engine.set_isolated_node(Some(mask_id));
    engine.render(0.0);
    let solo = engine.test_readback_canvas();
    let px = rgba_at(&solo, cw, cw / 2, ch / 2);
    assert!(
        (px[0] as i32 - px[1] as i32).abs() < 4 && (px[1] as i32 - px[2] as i32).abs() < 4,
        "isolated mask must render as RGB-equal grayscale (no red leak); \
         got {px:?}"
    );
    assert!(
        px[0] > 100 && px[0] < 160,
        "grayscale value should reflect mid-gray mask coverage (~128); \
         got {px:?}"
    );
    assert_eq!(
        px[3], 255,
        "isolated-mask grayscale output is fully opaque on canvas; got alpha={}",
        px[3]
    );
}

/// Translating a mask transform commits the moved pixels to the new
/// position and clears the source rect. Catches regressions in either
/// the commit shader (output value) or the engine's clear/save sequence
/// (which used to require a destructive setup-clear + un-clear dance).
#[test]
fn transform_translate_on_mask_moves_pixels() {
    use darkly::gpu::transform::affine_translate;
    let (cw, ch) = (32u32, 32u32);
    let mut engine = test_engine(cw, ch);
    let host = engine.add_raster_layer(None);
    fill_layer(&mut engine, host, 255, 255, 255);
    engine.add_mask(host);
    let mask_id = engine.host_mask_id(host).expect("host has mask");

    // Distinct fill so we can spot the moved pattern.
    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 200,
        g: 200,
        b: 200,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();

    let pre = engine.test_readback_mask(host);
    assert_eq!(pre[0], 200, "fixture: mask is filled to value 200");

    // Selection-driven path: select a small rect, then translate it.
    let sel_x = 8;
    let sel_y = 8;
    let sel_w = 8;
    let sel_h = 8;
    engine.select_rect(
        sel_x as f32,
        sel_y as f32,
        sel_w as f32,
        sel_h as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    let started = engine.begin_transform(mask_id);
    assert!(started, "begin_transform on mask must succeed");
    engine.update_floating_matrix(affine_translate(12.0, 0.0));
    engine.commit_floating();

    let post = engine.test_readback_mask(host);

    // New position (sel_x+12 .. sel_x+12+sel_w) should hold the moved value.
    let new_x = sel_x + 12;
    for dy in 0..sel_h {
        for dx in 0..sel_w {
            let v = post[((sel_y + dy) * cw + new_x + dx) as usize];
            assert!(
                v > 150,
                "moved pixel at ({}, {}) should carry the original mask value, got {v}",
                new_x + dx,
                sel_y + dy,
            );
        }
    }
    // Source rect cleared to 0 by the commit-time ClearShape application.
    for dy in 0..sel_h {
        for dx in 0..sel_w {
            let v = post[((sel_y + dy) * cw + sel_x + dx) as usize];
            assert_eq!(
                v,
                0,
                "source pixel at ({}, {}) should be cleared after commit, got {v}",
                sel_x + dx,
                sel_y + dy,
            );
        }
    }
    // Pixels outside the affected union stay at their original value.
    let untouched = post[((cw - 1) + cw * (ch - 1)) as usize];
    assert_eq!(
        untouched, 200,
        "pixels outside the affected rect must be unchanged"
    );
}

/// While a mask transform is active, the host's blend must read through
/// the *preview* texture so the mask's effect on the canvas reflects the
/// currently-dragged matrix. Specifically: if the user translated the
/// mask far away, the host pixels at the original mask position should
/// no longer be hidden by the (moved-away) mask coverage.
///
/// This is the regression for "during transform, mask effects vanish":
/// in the broken state, `setup_transform` destructively cleared the live
/// mask and the in-line preview pass skipped mask-target transforms, so
/// the host rendered with no mask at all.
#[test]
fn mask_visible_during_transform_drag() {
    use darkly::gpu::transform::affine_translate;
    let (cw, ch) = (32u32, 32u32);
    let mut engine = test_engine(cw, ch);
    let host = engine.add_raster_layer(None);
    fill_layer(&mut engine, host, 255, 0, 0);
    engine.add_mask(host);
    let mask_id = engine.host_mask_id(host).expect("host has mask");

    // Fresh masks default to fully visible (255). Black out the whole
    // mask first, then paint white into the left half so we have a sharp
    // 50/50 visibility boundary. Flood-fill is async — flush after each
    // submission so the next stroke sees its predecessor's pixels and
    // the selection state at *completion* time matches the intent.
    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 0,
        g: 0,
        b: 0,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();

    engine.select_rect(
        0.0,
        0.0,
        (cw / 2) as f32,
        ch as f32,
        SelectionMode::Replace,
        false,
        0.0,
    );
    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
    engine.clear_selection();
    engine.render(0.0);

    // Sanity: pre-transform mask is half white / half black.
    let mask_pre = engine.test_readback_mask(host);
    assert_eq!(mask_pre[0], 255, "left edge of mask should be 255");
    assert_eq!(
        mask_pre[(cw - 1) as usize],
        0,
        "right edge of mask should be 0"
    );
    let baseline = engine.test_readback_canvas();
    assert!(
        rgba_at(&baseline, cw, 4, ch / 2)[0] > 200,
        "left half should be opaque red; got {:?}",
        rgba_at(&baseline, cw, 4, ch / 2)
    );
    assert_eq!(
        rgba_at(&baseline, cw, cw - 4, ch / 2)[3],
        0,
        "right half should be transparent (mask=0)"
    );

    // Begin transform on the mask, translate by +cw/2 (mask shifts right).
    engine.select_all();
    let started = engine.begin_transform(mask_id);
    assert!(started, "begin_transform on mask must succeed");
    engine.update_floating_matrix(affine_translate((cw / 2) as f32, 0.0));
    engine.render(0.0);

    let dragging = engine.test_readback_canvas();
    // After the translate, the *preview* mask covers the right half. So:
    //   - left half should now be transparent (mask=0 there post-shift),
    //   - right half should now be opaque red (mask=255 there post-shift).
    // The broken state showed the left half still red because the live
    // mask was destructively cleared and the preview never ran.
    assert_eq!(
        rgba_at(&dragging, cw, 4, ch / 2)[3],
        0,
        "during drag, original mask position should be uncovered by the preview mask"
    );
    assert!(
        rgba_at(&dragging, cw, cw - 4, ch / 2)[0] > 200,
        "during drag, transformed mask position should reveal red"
    );

    engine.cancel_floating();
}

/// Cancel after a non-identity drag must leave the live mask texture
/// byte-for-byte identical to its pre-transform state. Under the old
/// architecture this required a `restore_from_scratch` round-trip; under
/// the derived-preview model the live texture was never touched, so cancel
/// reduces to dropping floating state.
#[test]
fn cancel_transform_on_mask_leaves_texture_pristine() {
    use darkly::gpu::transform::affine_translate;
    let (cw, ch) = (32u32, 32u32);
    let mut engine = test_engine(cw, ch);
    let host = engine.add_raster_layer(None);
    fill_layer(&mut engine, host, 255, 255, 255);
    engine.add_mask(host);
    let mask_id = engine.host_mask_id(host).expect("host has mask");
    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 191,
        g: 191,
        b: 191,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();

    let baseline = engine.test_readback_mask(host);

    engine.select_all();
    let started = engine.begin_transform(mask_id);
    assert!(started);
    engine.update_floating_matrix(affine_translate(7.0, 3.0));
    engine.cancel_floating();

    let after = engine.test_readback_mask(host);
    assert_eq!(
        after, baseline,
        "cancel must leave the mask exactly as it was pre-transform"
    );
}

/// Isolating a mask AND transforming it: the canvas must show the mask's
/// channel as grayscale at the *transformed* position. Verifies that the
/// preview indirection composes with the isolation render path — the host
/// renders with `isolated=true`, samples the preview-mask bind group, and
/// the shader's grayscale output reflects the moved mask shape.
#[test]
fn transform_mask_under_isolation_previews_grayscale() {
    use darkly::gpu::transform::affine_translate;
    let (cw, ch) = (32u32, 32u32);
    let mut engine = test_engine(cw, ch);
    let host = engine.add_raster_layer(None);
    // Host content is irrelevant (isolation hides it); fill with red so a
    // red leak in the assertion would be obvious.
    fill_layer(&mut engine, host, 255, 0, 0);
    engine.add_mask(host);
    let mask_id = engine.host_mask_id(host).expect("host has mask");

    // Mask: a small white square at (4..12, 4..12) on a black background.
    engine.select_rect(4.0, 4.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    engine.begin_stroke(mask_id);
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.clear_selection();
    engine.test_flush_readbacks();

    // Isolate the mask.
    engine.set_isolated_node(Some(mask_id));

    // Transform: translate the white square +16 pixels right.
    engine.select_rect(4.0, 4.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    let started = engine.begin_transform(mask_id);
    assert!(started);
    engine.update_floating_matrix(affine_translate(16.0, 0.0));
    engine.render(0.0);

    let canvas = engine.test_readback_canvas();
    // New position (20..28, 4..12) should be opaque grayscale white.
    let moved = rgba_at(&canvas, cw, 24, 8);
    assert!(
        moved[0] > 200 && moved[1] > 200 && moved[2] > 200 && moved[3] == 255,
        "transformed mask position should render as grayscale white through \
         the isolated-host shader path; got {moved:?}"
    );
    // RGB should be equal (grayscale) — no red leak from the host color.
    assert!(
        (moved[0] as i32 - moved[1] as i32).abs() < 4,
        "isolated-mask preview must be RGB-equal grayscale; got {moved:?}"
    );
    // Original square position is now black (mask shifted away from it).
    let original = rgba_at(&canvas, cw, 8, 8);
    assert!(
        original[0] < 32 && original[1] < 32 && original[2] < 32,
        "original mask position should be black after the preview shift; \
         got {original:?}"
    );

    engine.cancel_floating();
}

// ============================================================================
// Checkpoint ring — coverage invariant on a long stabilized stroke
// ============================================================================

/// Regression for the checkpoint ring coverage architecture.
///
/// Before the redesign, [`pick_slot`] (the eviction policy) destroyed the
/// lowest-vi anchor as soon as the ring filled, and `find_divergence` could
/// return values outside the advertised `max_divergence_window`. The two
/// defects compounded to ~2 mid-stroke full re-render fallbacks on long
/// high-stabilization strokes — each fallback re-renders the entire stroke
/// instead of the `O(window/8)` slice the architecture promises.
///
/// This test paints a long curving stroke at full Laplacian strength and
/// asserts that `full_rerender_events == 0`. With the redesign the
/// coverage invariant — at least one valid slot with
/// `vi < tip_vi − max_divergence_window` — holds after every save, so
/// `restore_before` always finds a checkpoint.
#[test]
fn long_stabilized_stroke_no_fallback() {
    let (w, h) = (512, 512);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Default brush (circle + stamp + color_output) is enough to exercise
    // the checkpoint ring's coverage invariant — this test is about the
    // stabilizer's full-rerender fallback, not anything scatter-specific.
    let pen_id = find_node_id(&engine, "pen_input");
    let stamp_id = find_node_id(&engine, "stamp");
    // Full-strength stabilization → max_divergence_window = 11 (iterations=10
    // + 1 from the influence-radius model). Spacing = 11 / 7 = 1.
    engine
        .brush_graph_set_port_default(pen_id, "stabilize", 1.0)
        .unwrap();
    // Smallish dab so the bbox stays cheap.
    engine
        .brush_graph_set_port_default(stamp_id, "size", 0.08)
        .unwrap();

    engine.begin_stroke(layer_id);
    // 400 samples along a slow spiral — enough to push the ring well past
    // `2 * max_divergence_window` (the failure threshold for defect 2). The
    // spiral exercises divergence on every event because relaxation keeps
    // shifting interior points as the tip continues to curve.
    let samples = 400usize;
    let cx = (w / 2) as f32;
    let cy = (h / 2) as f32;
    for i in 0..samples {
        let t = i as f32 / samples as f32;
        let theta = t * std::f32::consts::TAU * 3.0;
        let r = 20.0 + t * 200.0;
        let x = cx + r * theta.cos();
        let y = cy + r * theta.sin();
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y,
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

    assert_eq!(
        engine.test_stroke_full_rerender_events(),
        0,
        "long stabilized stroke must not trigger any mid-stroke full \
         re-render fallback — the coverage invariant guarantees \
         `restore_before` succeeds for every reachable divergence index"
    );
}

// ============================================================================
// Image export — async readback of the composited canvas
// ============================================================================

/// Verify that `start_export` → readback → `poll_export_result` produces
/// RGBA8 pixels that match the same bytes the test-only
/// `test_readback_canvas` returns from the composited texture. The async
/// path is what JS uses in production; the blocking path is what tests
/// use elsewhere — they must agree, or the production export is lying.
#[test]
fn export_readback_produces_rgba8_matching_composite() {
    let (cw, ch) = (64, 48);
    let mut engine = test_engine(cw, ch);
    let layer = engine.add_raster_layer(None);

    // Paint something distinctive so we're not comparing two empty canvases.
    paint_at(
        &mut engine,
        layer,
        (cw / 2) as f32,
        (ch / 2) as f32,
        1.0,
        0.3,
        0.0,
    );

    // Reference: the same composite path tests use elsewhere.
    let reference = engine.test_readback_canvas();
    assert_eq!(reference.len(), (cw * ch * 4) as usize);

    // Start the export and pump the frame loop until the result lands.
    engine.start_export();
    let mut result = None;
    for _ in 0..16 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if let Some(r) = engine.poll_export_result() {
            result = Some(r);
            break;
        }
    }
    let export = result.expect("export readback did not complete within 16 iterations");

    assert_eq!(export.width, cw);
    assert_eq!(export.height, ch);
    assert_eq!(
        export.rgba.len(),
        (cw * ch * 4) as usize,
        "export must be tightly packed RGBA8 with no row padding"
    );
    assert_eq!(
        export.rgba, reference,
        "async export bytes must equal the blocking composite readback — \
         the production export path would otherwise lie about canvas contents"
    );
}

/// A pending export readback returns `None` from `poll_export_result`
/// until completion. The frontend's per-frame poll relies on this for
/// "result not ready yet, try again next frame".
#[test]
fn poll_export_result_returns_none_before_completion() {
    let mut engine = test_engine(8, 8);
    let _layer = engine.add_raster_layer(None);

    engine.start_export();
    // No flush yet — the readback is queued but the GPU work isn't drained.
    assert!(
        engine.poll_export_result().is_none(),
        "result must not be available before the readback completes"
    );
}

/// Regression: locking a layer must block all subsequent mutations to it
/// (paint, rename, opacity, blend mode, delete, move). Originally only the
/// UI lock icon was wired — the engine accepted brush strokes against
/// locked layers because `Document::is_node_editable` did not exist.
#[test]
fn locked_layer_rejects_modifications() {
    let (w, h) = (64, 64);
    let mut engine = test_engine(w, h);
    // Keep two layers so `remove_layer`'s "last layer" guard never short-
    // circuits the lock check.
    let other = engine.add_raster_layer(None);
    let layer_id = engine.add_raster_layer(None);

    // Paint once unlocked so we have a baseline pixel set to compare against.
    paint_at(&mut engine, layer_id, 32.0, 32.0, 1.0, 0.0, 0.0);
    let baseline = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&baseline, w, 32, 32) > 0,
        "unlocked paint must land on the layer"
    );

    // Lock and capture the old metadata so we can prove the setters no-op.
    let old_name = "before-lock";
    engine.set_layer_name(layer_id, old_name);
    engine.set_opacity(layer_id, 0.5);
    engine.set_node_locked(layer_id, true);

    // 1. Paint is blocked: pixels must be byte-identical to baseline.
    paint_at(&mut engine, layer_id, 10.0, 10.0, 0.0, 1.0, 0.0);
    let after_paint = engine.test_readback_layer(layer_id);
    assert_eq!(
        baseline, after_paint,
        "locked layer must not accept any paint"
    );

    // 2. Property mutations are blocked. Read back through `layer_tree`,
    //    which is the same serialized view the UI sees, so we know what
    //    actually reaches users.
    engine.set_layer_name(layer_id, "should-be-ignored");
    engine.set_opacity(layer_id, 1.0);
    engine.set_blend_mode(layer_id, "multiply");
    let tree = engine.layer_tree();
    let info = tree
        .iter()
        .find_map(|n| match n {
            darkly::engine::types::LayerInfo::Raster {
                id,
                name,
                opacity,
                blend_mode,
                ..
            } if *id == layer_id.to_ffi() as f64 => Some((name.clone(), *opacity, *blend_mode)),
            _ => None,
        })
        .expect("layer in tree");
    assert_eq!(info.0, old_name, "rename must be rejected when locked");
    assert!(
        (info.1 - 0.5).abs() < 1e-6,
        "opacity change must be rejected when locked"
    );
    assert_eq!(
        info.2, "normal",
        "blend mode change must be rejected when locked"
    );

    // 3. Delete is rejected (other layer keeps it from being the "last").
    assert!(
        engine.remove_layer(layer_id).is_err(),
        "remove_layer must error when locked"
    );
    assert!(engine.has_layer(layer_id), "locked layer must still exist");

    // 4. Unlock and confirm paint flows again — proves the guard is the
    //    lock predicate and not some unrelated side-effect.
    engine.set_node_locked(layer_id, false);
    paint_at(&mut engine, layer_id, 10.0, 10.0, 0.0, 1.0, 0.0);
    let after_unlock = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&after_unlock, w, 10, 10) > 0,
        "paint must land again after unlock"
    );

    // Keep `other` referenced so the compiler doesn't warn about it.
    let _ = other;
}
