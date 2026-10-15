//! The colour selector's per-stroke jitter, end to end: the `colors.jitter*`
//! prefs are read at stroke start and applied to the foreground colour
//! through the same transform the `hsv` node uses, as a pure function of the
//! stroke seed. A pinned seed therefore replays the same colour, and zero
//! amplitudes leave the stroke bit-identical.

use darkly::brush::color_jitter::{self, ColorJitter};
use darkly::config::{self, ConfigValue};
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

const W: u32 = 128;
const H: u32 = 64;
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn set_jitter(hue: f64, saturation: f64, value: f64) {
    config::set(color_jitter::HUE_KEY, ConfigValue::Float(hue));
    config::set(color_jitter::SATURATION_KEY, ConfigValue::Float(saturation));
    config::set(color_jitter::VALUE_KEY, ConfigValue::Float(value));
}

fn paint_red(engine: &mut DarklyEngine, layer: LayerId) {
    engine.begin_stroke(layer).unwrap();
    for i in 0..20 {
        engine.stroke_to(StrokeOp::BrushStroke {
            x: 8.0 + i as f32 * ((W - 16) as f32 / 19.0),
            y: (H / 2) as f32,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
            cr: RED[0],
            cg: RED[1],
            cb: RED[2],
            ca: RED[3],
        });
    }
    engine.end_stroke();
    engine.render(0.0);
}

/// Paint one red stroke with the default brush under `seed` and read it back.
fn stroke_pixels(seed: u32) -> Vec<u8> {
    let (device, queue) = test_device();
    let mut engine = DarklyEngine::new(GpuContext::new_headless(device, queue), W, H);
    engine.set_stroke_seed(Some(seed));
    let layer = engine.add_raster_layer(None);
    paint_red(&mut engine, layer);
    engine.test_readback_layer(layer)
}

fn centre(pixels: &[u8]) -> [u8; 4] {
    let i = (((H / 2) * W + W / 2) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

#[test]
fn zero_amplitude_is_bit_identical() {
    let seed = 0x5eed;
    let with_defaults = stroke_pixels(seed);
    set_jitter(0.0, 0.0, 0.0);
    let with_explicit_zero = stroke_pixels(seed);
    assert!(
        with_defaults.chunks(4).any(|px| px[3] > 0),
        "the stroke painted"
    );
    assert_eq!(with_defaults, with_explicit_zero);
}

#[test]
fn jitter_is_a_pure_function_of_the_seed() {
    set_jitter(180.0, 0.0, 0.0);
    let seed = 0x5eed;
    let first = stroke_pixels(seed);
    assert_eq!(
        first,
        stroke_pixels(seed),
        "the same seed repaints the same colour"
    );
    assert_ne!(
        first,
        stroke_pixels(seed + 1),
        "another seed lands on another hue"
    );

    // The pixel is the transform applied once, at stroke start, to the
    // foreground: the same `apply` the engine ran.
    let px = centre(&first);
    assert!(px[3] >= 250, "centre is opaque, got alpha {}", px[3]);
    let want = ColorJitter {
        hue: 180.0,
        saturation: 0.0,
        value: 0.0,
    }
    .apply(RED, seed);
    for c in 0..3 {
        let got = px[c] as f32 / px[3] as f32;
        assert!(
            (got - want[c]).abs() <= 3.0 / 255.0,
            "channel {c}: gpu {got} vs {} (pixel {px:?}, want {want:?})",
            want[c]
        );
    }
}
