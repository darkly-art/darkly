//! Regression: flood fill after an upward canvas resize must not panic in the
//! undo commit, and undo/redo must round-trip the fill.
//!
//! `complete_flood_fill` committed `doc.canvas_rect()` (the resized canvas
//! window) against a scratch snapshot saved over the layer extent, which
//! `resize_canvas` leaves put in the plane. After expanding upward the canvas
//! rect is taller and offset above the layer, so `commit_region`'s containment
//! assert fired. The fill now claims the window, growing the layer to cover it
//! before the scratch snapshot is taken, so the commit is the diff of that
//! snapshot.
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

/// The layer's RGBA at plane `(x, y)`: its texel inside its texture,
/// transparent past it. The fill may grow the texture, so reads go by plane
/// position, never by the texture's own layout.
fn pixel_at(engine: &DarklyEngine, layer: LayerId, x: i32, y: i32) -> [u8; 4] {
    let ext = engine.test_node_extent(layer);
    if !ext.contains(CanvasRect::from_xywh(x, y, 1, 1)) {
        return [0; 4];
    }
    let px = engine.test_readback_layer(layer);
    let i = ((y - ext.y0()) as usize * ext.width as usize + (x - ext.x0()) as usize) * 4;
    px[i..i + 4].try_into().unwrap()
}

/// The layer's content over the canvas window, extent-independent.
fn window_content(engine: &DarklyEngine, layer: LayerId, window: CanvasRect) -> Vec<u8> {
    let mut out = Vec::new();
    for y in window.y0()..window.y1() {
        for x in window.x0()..window.x1() {
            out.extend_from_slice(&pixel_at(engine, layer, x, y));
        }
    }
    out
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
    // extent stays at (0, 0, 64×32); resize only moves the window in the plane.
    engine.resize_canvas(CanvasRect::from_xywh(0, -(h as i32), w, 2 * h));
    assert_eq!(engine.canvas_dimensions(), (w, 2 * h));

    // Fill inside the surviving layer extent (canvas y in [0, h)).
    flood_fill(&mut engine, layer, 32.0, 16.0, [0, 255, 0, 255]);

    // The seeded pixel is now opaque green, and so is the window area the
    // resize exposed above the layer: the layer's (transparent) content there
    // is connected to the seed.
    assert_eq!(pixel_at(&engine, layer, 32, 16), [0, 255, 0, 255]);
    assert_eq!(pixel_at(&engine, layer, 32, -16), [0, 255, 0, 255]);
}

/// Undo restores the pre-fill pixels; redo re-applies the fill. Exercises the
/// commit's readback pipeline end to end for the smaller-than-canvas layer.
#[test]
fn flood_fill_after_upward_resize_undo_redo_round_trips() {
    let (w, h) = (64u32, 32u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);
    let window = CanvasRect::from_xywh(0, -(h as i32), w, 2 * h);
    engine.resize_canvas(window);

    let before = window_content(&engine, layer, window);
    flood_fill(&mut engine, layer, 32.0, 16.0, [0, 255, 0, 255]);
    let filled = window_content(&engine, layer, window);
    assert_ne!(before, filled, "fill must change the layer pixels");

    // Let the undo region entry's readback reach `Ready` before restoring.
    for _ in 0..16 {
        engine.test_flush_readbacks();
    }
    engine.undo();
    engine.render(0.0);
    let after_undo = window_content(&engine, layer, window);
    assert_eq!(
        after_undo, before,
        "undo must restore the pre-fill layer pixels"
    );

    engine.redo();
    engine.render(0.0);
    engine.test_flush_readbacks();
    let after_redo = window_content(&engine, layer, window);
    assert_eq!(after_redo, filled, "redo must re-apply the fill");
}
