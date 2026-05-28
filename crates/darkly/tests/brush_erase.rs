//! Regression test for brush-tool erase mode (the user-facing E toggle).
//!
//! Drives a real `DarklyEngine` end-to-end: fill a layer red, flip
//! `set_brush_blend_mode(1)`, stroke across the centre, and assert the
//! stroke path lost alpha while off-stroke pixels stayed solid red.
//!
//! This is the test that was missing when the `paint` terminal rewrite
//! silently broke erase. The other "erase" tests in the suite
//! (`paint_target_erase_circle`, `gpu_erase_stroke_undo`) exercise the
//! dead `GpuPaintTarget::erase_circle` helper, not the brush stroke path.
//!
//! Run with: `cargo test -p darkly --test brush_erase -- --test-threads=1`
//! (GPU integration tests share a process-wide wgpu device.)
//!
//! Per CLAUDE.md's Testing Principle: confirm this test FAILS against the
//! unfixed `paint.rs` (per-dab `erase_pipeline` branch leaves the scratch
//! at zero, so `destination_out` is a no-op), then passes after removing
//! that branch.

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
    engine.test_flush_readbacks();
}

fn paint_stroke_across(engine: &mut DarklyEngine, layer_id: LayerId, w: u32, h: u32) {
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
            cg: 1.0,
            cb: 1.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
    engine.test_flush_readbacks();
}

fn rgba_at(pixels: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Brush erase removes coverage from the layer along the stroke path
/// while leaving off-path pixels untouched.
#[test]
fn brush_stroke_in_erase_mode_lowers_alpha_on_path() {
    let (w, h) = (128u32, 128u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    fill_layer(&mut engine, layer_id, 255, 0, 0);

    // Sanity: the off-path corner is solid red BEFORE we start erasing.
    // If this fails the test setup is broken, not the feature under test.
    let pre = engine.test_readback_layer(layer_id);
    assert_eq!(
        rgba_at(&pre, w, 4, 4),
        [255, 0, 0, 255],
        "fill_layer should produce solid red before the erase stroke runs"
    );

    engine.set_brush_blend_mode(1);
    paint_stroke_across(&mut engine, layer_id, w, h);

    let post = engine.test_readback_layer(layer_id);

    // On-path pixel must have lost alpha. The exact value depends on the
    // default brush's flow/opacity/spacing, so we just assert
    // meaningful erosion rather than a hard `== 0`.
    let on_path = rgba_at(&post, w, w / 2, h / 2);
    assert!(
        on_path[3] < 200,
        "pixel under the erase stroke should have alpha < 200, got {on_path:?}"
    );

    // Off-path pixel (far from the y = h/2 stroke) must remain solid red.
    let off_path = rgba_at(&post, w, 4, 4);
    assert_eq!(
        off_path,
        [255, 0, 0, 255],
        "pixel off the erase stroke path must remain solid red, got {off_path:?}"
    );
}

/// Toggling erase off again restores normal paint behaviour. Guards
/// against a fix that hard-codes the per-dab pass to one mode.
#[test]
fn brush_stroke_paints_normally_after_disabling_erase_mode() {
    let (w, h) = (128u32, 128u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Erase mode on, then off — the layer starts empty so the erase
    // stroke is a no-op; we're checking the toggle round-trip.
    engine.set_brush_blend_mode(1);
    engine.set_brush_blend_mode(0);

    paint_stroke_across(&mut engine, layer_id, w, h);

    let pixels = engine.test_readback_layer(layer_id);
    let on_path = rgba_at(&pixels, w, w / 2, h / 2);
    assert!(
        on_path[3] > 50,
        "after toggling erase off, a paint stroke must deposit alpha, got {on_path:?}"
    );
}
