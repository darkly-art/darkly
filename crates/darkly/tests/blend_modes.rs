//! Per-mode regression tests for layer blend modes.
//!
//! Each test fills a bg + fg layer with known colors, sets the fg's blend
//! mode, renders the composite, and verifies the output pixel matches the
//! formula computed in Rust. Defends against shader edits silently breaking
//! individual modes. Reference formulas mirror Krita's
//! `KoCompositeOpFunctions.h` line-for-line.
//!
//! Run with: `cargo test -p darkly --test blend_modes`

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::BlendMode;

const W: u32 = 4;
const H: u32 = 4;

// Test colors: non-symmetric (direction matters), away from 0/1 boundaries
// (exercises non-edge formula paths cleanly), distinct enough to detect
// channel-mixing bugs.
const FG_RGBA: [u8; 4] = [200, 80, 160, 255];
const BG_RGBA: [u8; 4] = [100, 180, 90, 255];

// ±2 byte tolerance — accommodates u8↔f32↔u8 round-trip and minor float
// rounding between WGSL and Rust.
const EPSILON: f32 = 2.0 / 255.0;

fn fg() -> [f32; 3] {
    [
        FG_RGBA[0] as f32 / 255.0,
        FG_RGBA[1] as f32 / 255.0,
        FG_RGBA[2] as f32 / 255.0,
    ]
}
fn bg() -> [f32; 3] {
    [
        BG_RGBA[0] as f32 / 255.0,
        BG_RGBA[1] as f32 / 255.0,
        BG_RGBA[2] as f32 / 255.0,
    ]
}

fn test_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, W, H)
}

fn solid_rgba(c: [u8; 4]) -> Vec<u8> {
    let mut v = Vec::with_capacity((W * H * 4) as usize);
    for _ in 0..(W * H) {
        v.extend_from_slice(&c);
    }
    v
}

fn render_blend(mode: BlendMode) -> [f32; 3] {
    let mut engine = test_engine();
    let bg_id = engine.paste_image(W, H, &solid_rgba(BG_RGBA), 0, 0, None);
    let fg_id = engine.paste_image(W, H, &solid_rgba(FG_RGBA), 0, 0, Some(bg_id));
    engine.set_blend_mode(fg_id, mode as u32);
    let pixels = engine.test_readback_canvas();
    let center = ((H / 2) * W + (W / 2)) as usize * 4;
    [
        pixels[center] as f32 / 255.0,
        pixels[center + 1] as f32 / 255.0,
        pixels[center + 2] as f32 / 255.0,
    ]
}

fn assert_close(actual: [f32; 3], expected: [f32; 3], mode: BlendMode) {
    for i in 0..3 {
        let diff = (actual[i] - expected[i]).abs();
        assert!(
            diff < EPSILON,
            "{:?} channel {}: actual {:.4} expected {:.4} diff {:.4}",
            mode,
            i,
            actual[i],
            expected[i],
            diff,
        );
    }
}

// ---------- Reference formulas (mirror composite.wgsl) ----------

fn rf_normal(s: [f32; 3], _d: [f32; 3]) -> [f32; 3] {
    s
}
fn rf_darken(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    [s[0].min(d[0]), s[1].min(d[1]), s[2].min(d[2])]
}
fn rf_multiply(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    [s[0] * d[0], s[1] * d[1], s[2] * d[2]]
}
fn rf_color_burn(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = if d[i] >= 1.0 {
            1.0
        } else if s[i] <= 0.0 {
            0.0
        } else {
            (1.0 - (1.0 - d[i]) / s[i]).clamp(0.0, 1.0)
        };
    }
    out
}
fn rf_lighten(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    [s[0].max(d[0]), s[1].max(d[1]), s[2].max(d[2])]
}
fn rf_screen(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    [
        s[0] + d[0] - s[0] * d[0],
        s[1] + d[1] - s[1] * d[1],
        s[2] + d[2] - s[2] * d[2],
    ]
}
fn rf_color_dodge(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = if s[i] >= 1.0 {
            if d[i] > 0.0 {
                1.0
            } else {
                0.0
            }
        } else {
            (d[i] / (1.0 - s[i])).clamp(0.0, 1.0)
        };
    }
    out
}
fn rf_linear_dodge(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    [
        (s[0] + d[0]).clamp(0.0, 1.0),
        (s[1] + d[1]).clamp(0.0, 1.0),
        (s[2] + d[2]).clamp(0.0, 1.0),
    ]
}
fn rf_overlay(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = if d[i] < 0.5 {
            2.0 * s[i] * d[i]
        } else {
            1.0 - 2.0 * (1.0 - s[i]) * (1.0 - d[i])
        };
    }
    out
}
fn rf_soft_light(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = if s[i] > 0.5 {
            d[i] + (2.0 * s[i] - 1.0) * (d[i].sqrt() - d[i])
        } else {
            d[i] - (1.0 - 2.0 * s[i]) * d[i] * (1.0 - d[i])
        };
    }
    out
}
fn rf_hard_light(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = if s[i] <= 0.5 {
            2.0 * s[i] * d[i]
        } else {
            1.0 - 2.0 * (1.0 - s[i]) * (1.0 - d[i])
        };
    }
    out
}
fn rf_difference(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    [
        (s[0] - d[0]).abs(),
        (s[1] - d[1]).abs(),
        (s[2] - d[2]).abs(),
    ]
}

fn lum(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}
fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        for v in &mut out {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 {
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    out
}
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}
fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let cmax = c[0].max(c[1]).max(c[2]);
    let cmin = c[0].min(c[1]).min(c[2]);
    let range = cmax - cmin;
    if range <= 0.0 {
        return [0.0; 3];
    }
    let scale = s / range;
    [
        (c[0] - cmin) * scale,
        (c[1] - cmin) * scale,
        (c[2] - cmin) * scale,
    ]
}
fn rf_hue(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    set_lum(set_sat(s, sat(d)), lum(d))
}
fn rf_saturation(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    set_lum(set_sat(d, sat(s)), lum(d))
}
fn rf_color(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    set_lum(s, lum(d))
}
fn rf_luminosity(s: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    set_lum(d, lum(s))
}

// ---------- Tests (one per mode) ----------

#[test]
fn blend_normal() {
    assert_close(
        render_blend(BlendMode::Normal),
        rf_normal(fg(), bg()),
        BlendMode::Normal,
    );
}

#[test]
fn blend_darken() {
    assert_close(
        render_blend(BlendMode::Darken),
        rf_darken(fg(), bg()),
        BlendMode::Darken,
    );
}

#[test]
fn blend_multiply() {
    assert_close(
        render_blend(BlendMode::Multiply),
        rf_multiply(fg(), bg()),
        BlendMode::Multiply,
    );
}

#[test]
fn blend_color_burn() {
    assert_close(
        render_blend(BlendMode::ColorBurn),
        rf_color_burn(fg(), bg()),
        BlendMode::ColorBurn,
    );
}

#[test]
fn blend_lighten() {
    assert_close(
        render_blend(BlendMode::Lighten),
        rf_lighten(fg(), bg()),
        BlendMode::Lighten,
    );
}

#[test]
fn blend_screen() {
    assert_close(
        render_blend(BlendMode::Screen),
        rf_screen(fg(), bg()),
        BlendMode::Screen,
    );
}

#[test]
fn blend_color_dodge() {
    assert_close(
        render_blend(BlendMode::ColorDodge),
        rf_color_dodge(fg(), bg()),
        BlendMode::ColorDodge,
    );
}

#[test]
fn blend_linear_dodge() {
    assert_close(
        render_blend(BlendMode::LinearDodge),
        rf_linear_dodge(fg(), bg()),
        BlendMode::LinearDodge,
    );
}

#[test]
fn blend_overlay() {
    assert_close(
        render_blend(BlendMode::Overlay),
        rf_overlay(fg(), bg()),
        BlendMode::Overlay,
    );
}

#[test]
fn blend_soft_light() {
    assert_close(
        render_blend(BlendMode::SoftLight),
        rf_soft_light(fg(), bg()),
        BlendMode::SoftLight,
    );
}

#[test]
fn blend_hard_light() {
    assert_close(
        render_blend(BlendMode::HardLight),
        rf_hard_light(fg(), bg()),
        BlendMode::HardLight,
    );
}

#[test]
fn blend_difference() {
    assert_close(
        render_blend(BlendMode::Difference),
        rf_difference(fg(), bg()),
        BlendMode::Difference,
    );
}

#[test]
fn blend_hue() {
    assert_close(
        render_blend(BlendMode::Hue),
        rf_hue(fg(), bg()),
        BlendMode::Hue,
    );
}

#[test]
fn blend_saturation() {
    assert_close(
        render_blend(BlendMode::Saturation),
        rf_saturation(fg(), bg()),
        BlendMode::Saturation,
    );
}

#[test]
fn blend_color() {
    assert_close(
        render_blend(BlendMode::Color),
        rf_color(fg(), bg()),
        BlendMode::Color,
    );
}

#[test]
fn blend_luminosity() {
    assert_close(
        render_blend(BlendMode::Luminosity),
        rf_luminosity(fg(), bg()),
        BlendMode::Luminosity,
    );
}
