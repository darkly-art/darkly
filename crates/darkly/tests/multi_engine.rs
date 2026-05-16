//! Two `DarklyEngine` instances on a single shared `GpuDevice`.
//!
//! Confirms the multi-tab foundation: many engines can render against one
//! WebGPU device without state cross-talk. Painting on engine A must not be
//! visible from engine B's readback, and vice versa.
//!
//! Run with: `cargo test -p darkly --test multi_engine -- --test-threads=1`

use std::sync::Arc;

use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::{GpuContext, GpuDevice};
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn paint_dot(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32, r: f32, g: f32, b: f32) {
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
    engine.render(0.0);
}

fn alpha_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[((y * w + x) * 4 + 3) as usize]
}

#[test]
fn two_engines_share_one_device_without_crosstalk() {
    let (device, queue) = test_device();
    let shared = Arc::new(GpuDevice { device, queue });

    let gpu_a = GpuContext::new_headless_shared(Arc::clone(&shared));
    let gpu_b = GpuContext::new_headless_shared(Arc::clone(&shared));

    // Confirm both contexts point at the same underlying device — that's the
    // whole point. If `Arc::strong_count` shows 3 (this fn + two contexts),
    // the device is genuinely shared.
    assert_eq!(Arc::strong_count(&shared), 3);

    let (w, h) = (64u32, 64u32);
    let mut engine_a = DarklyEngine::new(gpu_a, w, h);
    let mut engine_b = DarklyEngine::new(gpu_b, w, h);

    let layer_a = engine_a.add_raster_layer(None);
    let layer_b = engine_b.add_raster_layer(None);

    // Paint to engine A only.
    paint_dot(&mut engine_a, layer_a, 16.0, 16.0, 1.0, 0.0, 0.0);

    let pixels_a = engine_a.test_readback_layer(layer_a);
    let pixels_b = engine_b.test_readback_layer(layer_b);

    assert!(
        alpha_at(&pixels_a, w, 16, 16) > 0,
        "engine A's stroke should be visible in A"
    );
    assert_eq!(
        alpha_at(&pixels_b, w, 16, 16),
        0,
        "engine A's stroke must NOT leak into engine B"
    );

    // Paint to engine B at a different spot, confirm A is unaffected.
    paint_dot(&mut engine_b, layer_b, 48.0, 48.0, 0.0, 1.0, 0.0);

    let pixels_a2 = engine_a.test_readback_layer(layer_a);
    let pixels_b2 = engine_b.test_readback_layer(layer_b);

    assert!(
        alpha_at(&pixels_b2, w, 48, 48) > 0,
        "engine B's stroke should be visible in B"
    );
    assert_eq!(
        alpha_at(&pixels_a2, w, 48, 48),
        0,
        "engine B's stroke must NOT leak into engine A"
    );
    // A's earlier stroke survives B's painting.
    assert!(
        alpha_at(&pixels_a2, w, 16, 16) > 0,
        "engine A's earlier stroke should still be there after engine B painted"
    );
}

#[test]
fn dropping_one_engine_keeps_the_other_alive() {
    // Regression guard: if one engine is closed (e.g. a tab is closed), the
    // other must keep working. Specifically, the shared device must outlive
    // the dropped engine because the surviving engine still holds an Arc.

    let (device, queue) = test_device();
    let shared = Arc::new(GpuDevice { device, queue });

    let gpu_a = GpuContext::new_headless_shared(Arc::clone(&shared));
    let gpu_b = GpuContext::new_headless_shared(Arc::clone(&shared));

    let (w, h) = (32u32, 32u32);
    let engine_a = DarklyEngine::new(gpu_a, w, h);
    let mut engine_b = DarklyEngine::new(gpu_b, w, h);

    // Drop A's outer Arc on the device too.
    drop(engine_a);
    drop(shared);

    let layer_b = engine_b.add_raster_layer(None);
    paint_dot(&mut engine_b, layer_b, 8.0, 8.0, 0.0, 0.0, 1.0);

    let pixels_b = engine_b.test_readback_layer(layer_b);
    assert!(
        alpha_at(&pixels_b, w, 8, 8) > 0,
        "engine B should still render after engine A and the outer device handle dropped"
    );
}
