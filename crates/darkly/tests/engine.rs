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
use darkly::nodegraph::NodeInstance;

/// Paint a solid-color brush stroke at a given position (test helper replacing legacy PaintCircle).
fn paint_at(engine: &mut DarklyEngine, layer_id: u64, x: f32, y: f32, r: f32, g: f32, b: f32) {
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

// ============================================================================
// Brush stroke respects selection
// ============================================================================

#[test]
fn engine_brush_stroke_respects_selection() {
    let (w, h) = (128, 128);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

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
    let layer_id = engine.add_raster_layer();

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
    let base_layer = engine.add_raster_layer();

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
    let _base = engine.add_raster_layer();

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

/// Regression for the canvas-clamping bug: pasting an image larger than
/// the canvas must preserve the full extent on the layer, not crop to
/// canvas dimensions.
#[test]
fn paste_image_floating_preserves_off_canvas_extent() {
    use darkly::layer::LayerBounds;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer();

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
        LayerBounds {
            offset_x: ox,
            offset_y: oy,
            width: pw,
            height: ph,
        },
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
    use darkly::layer::LayerBounds;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer();

    let pw: u32 = 200;
    let ph: u32 = 100;
    let rgba = vec![0x44u8; (pw * ph * 4) as usize];

    let pasted_id = engine.paste_image(pw, ph, &rgba, -50, 10, None);

    let bounds = engine
        .layer_bounds(pasted_id)
        .expect("pasted layer must have bounds");
    assert_eq!(
        bounds,
        LayerBounds {
            offset_x: -50,
            offset_y: 10,
            width: pw,
            height: ph,
        },
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
    let base_layer = engine.add_raster_layer();

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
    let base_layer = engine.add_raster_layer();

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

// ============================================================================
// Scatter brush dabs must survive stabilizer-driven checkpoint restore
// ============================================================================

fn find_node_id(engine: &DarklyEngine, type_id: &str) -> u64 {
    engine
        .active_brush_graph_ref()
        .nodes
        .values()
        .find(|n: &&NodeInstance<BrushWireType>| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in default graph"))
        .id
        .0
}

/// Regression: `stroke_engine::place_dab` used to derive the save-point
/// bbox from `info.pos ± dab_radius` — the unscattered polyline point, not
/// where the dab actually landed. Every graph that offsets the dab (scatter
/// being the obvious one) dropped paint outside the recorded bbox on
/// checkpoint restore. With the stabilizer enabled, checkpoints save every
/// `spacing` dabs and the synthetic tip divergence fires on every pen
/// event, so the drop happens continuously during live drawing.
///
/// Setup: loads the "Scatter Brush" built-in (scatter node on the position
/// wire, size-proportional via `stamp.dab_size`). Amount_y is forced
/// high and the scatter node's own random is deterministic (hash of
/// `stroke_seed + node_id + dab_index`), so replays reproduce the same
/// pattern. With the bug, pixels outside the unscattered bbox are wiped
/// on each checkpoint restore; with the fix, they survive.
#[test]
fn scatter_brush_survives_checkpoint_restore() {
    let (w, h) = (256, 256);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();

    engine.brush_load("Scatter Brush").expect("brush load");

    // Configure the scatter brush graph to exercise the checkpoint path.
    let pen_id = find_node_id(&engine, "pen_input");
    let stamp_id = find_node_id(&engine, "stamp");
    let scatter_id = find_node_id(&engine, "scatter");
    // Enable laplacian stabilizer — gives spacing > 1 so restores actually
    // find a prior checkpoint (with spacing=1, restore_before's strict `<`
    // test never matches and the bug is hidden behind a full re-render).
    engine
        .brush_graph_set_port_default(pen_id, "stabilize", 0.5)
        .unwrap();
    // Pin dab size: pressure(=1) → stamp.size_input, size=0.1 → ~51px dab at
    // MAX_DAB_SIZE=512. amount_y=1.0 offsets up to ±51px per dab.
    engine
        .brush_graph_set_port_default(stamp_id, "size", 0.1)
        .unwrap();
    engine
        .brush_graph_set_port_default(scatter_id, "amount_x", 0.0)
        .unwrap();
    engine
        .brush_graph_set_port_default(scatter_id, "amount_y", 1.0)
        .unwrap();

    // Horizontal stroke at y=128. With scatter, every dab lands centered
    // near y=174 (=128 + 0.9 * 51), footprint y ≈ [148, 200].
    let stroke_y = (h / 2) as f32;
    engine.begin_stroke(layer_id);
    let samples = 40;
    for i in 0..samples {
        let t = i as f32 / (samples - 1) as f32;
        let x = 32.0 + t * (w as f32 - 64.0);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: stroke_y,
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

    let pixels = engine.test_readback_layer(layer_id);

    // Measure the vertical spread of painted pixels. With scatter on Y,
    // paint should spread well past the unscattered bbox around y=128
    // (which would be ~y ∈ [102, 154], total ~52px tall for this dab
    // size). The bug clamps the spread to that bbox; the fix preserves
    // the full scattered footprint ~y ∈ [51, 205], total ~150px tall.
    let mut min_y = u32::MAX;
    let mut max_y = 0u32;
    for py in 0..h {
        for px in 0..w {
            if alpha_at(&pixels, w, px, py) > 0 {
                min_y = min_y.min(py);
                max_y = max_y.max(py);
            }
        }
    }
    assert!(min_y != u32::MAX, "stroke painted nothing");
    let spread = max_y - min_y + 1;
    assert!(
        spread > 90,
        "scatter vertical spread is only {spread}px (y ∈ [{min_y}, {max_y}]); \
         dab footprint should stretch ~150px across y but is clamped to the \
         unscattered bbox because checkpoint restore wipes outside-bbox pixels"
    );
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

fn paint_horizontal_stroke(engine: &mut DarklyEngine, layer_id: u64, w: u32, h: u32) {
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
    let layer_id = engine.add_raster_layer();
    let pen_id = find_node_id(&engine, "pen_input");
    engine
        .brush_graph_set_port_default(pen_id, "spacing", 0.10)
        .expect("default spacing port must exist");
    paint_horizontal_stroke(&mut engine, layer_id, w, h);
    let dense_alpha = alpha_sum(&engine.test_readback_layer(layer_id), w, h);

    // Sparse: 100% spacing — dabs separated by a full diameter.
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer();
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
/// canvas-pos interpreted as layer-local. Regression for P1b.4 brush
/// composite shader migration.
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
// P2 — Brush strokes grow the layer (Phase 2B)
// ============================================================================

/// Brush stroke whose center falls past the canvas right edge must extend
/// the layer's canvas extent rightward by at least one growth chunk
/// (256-pixel multiple), preserving the originally-allocated content.
#[test]
fn brush_stroke_off_canvas_grows_layer() {
    let (cw, ch) = (256u32, 256u32);
    let mut engine = test_engine(cw, ch);
    let layer_id = engine.add_raster_layer();

    let bounds_before = engine.layer_bounds(layer_id).expect("layer exists");
    assert_eq!(bounds_before.offset_x, 0);
    assert_eq!(bounds_before.offset_y, 0);
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
        bounds_after.offset_x, 0,
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
    let layer_id = engine.add_raster_layer();

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

    let lx = (canvas_x - bounds.offset_x) as u32;
    let ly = (canvas_y - bounds.offset_y) as u32;
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
    let layer_id = engine.add_raster_layer();

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
        bounds.offset_x <= -256,
        "negative-direction growth should shift offset_x by at least one chunk; got {}",
        bounds.offset_x
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
    let layer_id = engine.add_raster_layer();

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
        bounds.offset_y <= -256,
        "negative-direction Y growth should shift offset_y by at least one chunk; got {}",
        bounds.offset_y
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
    let layer_id = engine.add_raster_layer();

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
    let layer_id = engine.add_raster_layer();

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
        let new_x = x as i32 + (pre_bounds.offset_x - after_bounds.offset_x);
        let new_y = 64i32 + (pre_bounds.offset_y - after_bounds.offset_y);
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
    let layer_id = engine.add_raster_layer();

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
    let layer_id = engine.add_raster_layer();

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
    let lx = (100 - after_bounds.offset_x) as u32;
    let ly = (100 - after_bounds.offset_y) as u32;
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
    use darkly::engine::types::LayerInfo;
    use darkly::layer::LayerBounds;

    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer();

    // Paste 200×200 at (-50, -50) — paste-extent layer with bounds that
    // extend in both negative-canvas directions and past the canvas.
    let pw: u32 = 200;
    let ph: u32 = 200;
    let rgba = vec![0x33u8; (pw * ph * 4) as usize];
    let pasted_id = engine.paste_image(pw, ph, &rgba, -50, -50, None);

    // Walk the engine's layer tree and find the pasted layer's info.
    let tree = engine.layer_tree();
    let mut found_bounds: Option<LayerBounds> = None;
    for info in &tree {
        if let LayerInfo::Raster { id, bounds, .. } = info {
            if *id as u64 == pasted_id {
                found_bounds = Some(*bounds);
                break;
            }
        }
    }
    let bounds = found_bounds.expect("pasted layer must appear in layer_tree as Raster");
    assert_eq!(
        bounds,
        LayerBounds {
            offset_x: -50,
            offset_y: -50,
            width: pw,
            height: ph,
        },
        "LayerInfo bounds must reflect the actual paste extent"
    );

    // Round-trip the bounds field through serde to confirm the FFI
    // serialization preserves the canvas-space offsets and dimensions.
    let json = serde_json::to_string(&bounds).expect("bounds must serialize");
    let decoded: LayerBounds =
        serde_json::from_str(&json).expect("bounds must deserialize byte-identically");
    assert_eq!(decoded, bounds);
    // Frontend-facing camelCase contract — keys end up as offsetX/offsetY.
    assert!(
        json.contains("\"offsetX\":-50"),
        "expected camelCase JSON keys, got {json}"
    );
}

/// Repeated paste → cancel cycles must not leak GPU textures. Regression
/// for P3: `cancel_floating` on the auto-created paste layer disposes its
/// compositor state in addition to detaching the doc node.
#[test]
fn paste_cancel_cycles_dont_leak_layer_textures() {
    let (cw, ch) = (64, 64);
    let mut engine = test_engine(cw, ch);
    let _base = engine.add_raster_layer();

    let baseline = engine.test_layer_texture_count();

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

    let after_cycles = engine.test_layer_texture_count();
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
    let _base = engine.add_raster_layer();

    let baseline = engine.test_layer_texture_count();

    for _ in 0..5 {
        let id = engine.add_raster_layer();
        assert!(engine.has_layer(id));
        engine.remove_layer(id).expect("remove should succeed");
        assert!(!engine.has_layer(id));
    }

    let after_cycles = engine.test_layer_texture_count();
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
    let layer_id = engine.add_raster_layer();
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
// Floating undo on offset / paste-extent layers (typed-coord refactor)
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
