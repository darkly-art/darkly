//! Regression: flood fill after an upward canvas resize must not panic in the
//! undo commit, and undo/redo must round-trip the fill.
//!
//! `complete_flood_fill` committed `doc.canvas_rect()` (the resized canvas
//! window) against a scratch snapshot saved over the layer extent, which
//! `resize_canvas` leaves put in the plane. After expanding upward the canvas
//! rect is taller and offset above the layer, so `commit_region`'s containment
//! assert fired. The fill can only touch the layer texture, so the layer extent
//! is the correct commit rect.
//!
//! Run with: cargo test -p darkly --test flood_fill_resize --features darkly/testing -- --test-threads=1

use darkly::coord::CanvasRect;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn flood_fill(engine: &mut DarklyEngine, layer: LayerId, x: f32, y: f32, color: [u8; 4]) {
    engine.begin_stroke(layer).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x,
        y,
        r: color[0],
        g: color[1],
        b: color[2],
        a: color[3],
        tolerance: 0,
    });
    engine.end_stroke();
    // Flood fill is async: the readback → `complete_flood_fill` (where the
    // commit lives) fires on flush. This is the line that panics against the
    // unfixed code.
    engine.test_flush_readbacks();
    engine.render(0.0);
}

/// Fill a layer whose texture extent is smaller than the canvas window after an
/// upward resize. The commit must stay within the saved (layer-extent) snapshot.
#[test]
fn flood_fill_after_upward_resize_does_not_panic() {
    // Small doc; a fresh raster layer's texture is sized to the canvas: (0,0,64×32).
    let (w, h) = (64u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);

    // Expand upward, doubling height: canvas window (0, -32, 64×64); the layer
    // extent stays at (0, 0, 64×32) — resize only moves the window in the plane.
    engine.resize_canvas(CanvasRect::from_xywh(0, -(h as i32), w, 2 * h));
    assert_eq!(engine.canvas_dimensions(), (w, 2 * h));

    // Fill inside the surviving layer extent (canvas y in [0, h)).
    flood_fill(&mut engine, layer, 32.0, 16.0, [0, 255, 0, 255]);

    // The seeded pixel (layer-local (32,16)) is now opaque green.
    let px = engine.test_readback_layer(layer);
    let idx = ((16u32 * w + 32u32) * 4) as usize;
    assert!(
        px[idx] < 50 && px[idx + 1] > 200 && px[idx + 3] > 200,
        "seed pixel must be opaque green, got ({},{},{},{})",
        px[idx],
        px[idx + 1],
        px[idx + 2],
        px[idx + 3]
    );
}

/// Undo restores the pre-fill pixels; redo re-applies the fill. Exercises the
/// commit's readback pipeline end to end for the smaller-than-canvas layer.
#[test]
fn flood_fill_after_upward_resize_undo_redo_round_trips() {
    let (w, h) = (64u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);
    engine.resize_canvas(CanvasRect::from_xywh(0, -(h as i32), w, 2 * h));

    let before = engine.test_readback_layer(layer);
    flood_fill(&mut engine, layer, 32.0, 16.0, [0, 255, 0, 255]);
    let filled = engine.test_readback_layer(layer);
    assert_ne!(before, filled, "fill must change the layer pixels");

    // Let the undo region entry's readback reach `Ready` before restoring.
    for _ in 0..16 {
        engine.test_flush_readbacks();
    }
    engine.undo();
    engine.render(0.0);
    let after_undo = engine.test_readback_layer(layer);
    assert_eq!(
        after_undo, before,
        "undo must restore the pre-fill layer pixels"
    );

    engine.redo();
    engine.render(0.0);
    engine.test_flush_readbacks();
    let after_redo = engine.test_readback_layer(layer);
    assert_eq!(after_redo, filled, "redo must re-apply the fill");
}
