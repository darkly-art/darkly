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
/// Setup: loads the "Scatter Brush" preset (scatter node on the position
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

    engine
        .brush_preset_load("Scatter Brush")
        .expect("preset load");

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
    // Pin dab size: pressure(=1) → stamp.size, scale=0.1 → ~51px dab at
    // MAX_DAB_SIZE=512. amount_y=1.0 offsets up to ±51px per dab.
    engine
        .brush_graph_set_port_default(stamp_id, "scale", 0.1)
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
