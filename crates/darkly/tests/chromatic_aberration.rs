//! Destructive chromatic-aberration integration tests — the CA filter over the
//! shared `filter_node_region` substrate, exercising the new `SrcSampling::
//! Bilinear` source mode and the List/Color/Vec2 param kinds end to end.
//!
//! Run with: `cargo test -p darkly --test chromatic_aberration --features testing -- --test-threads=1`

use std::collections::BTreeMap;

use darkly::document::SelectionMode;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::params::ParamValue;
use darkly::gpu::test_utils::*;
use darkly::gpu::veil::VeilRegistry;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// A `w`×`h` opaque RGBA buffer with distinct per-pixel values that vary in both
/// x and y, so a horizontal shift is detectable.
fn distinct_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            v[i] = (x * 13 + 5) as u8;
            v[i + 1] = (y * 11 + 9) as u8;
            v[i + 2] = ((x + y) * 7 + 3) as u8;
            v[i + 3] = 255;
        }
    }
    v
}

fn px(buf: &[u8], stride: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * stride + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// Build one aberration entry as the `{ name: value }` map the List param uses.
fn entry(offset: [f32; 2], scale: f32, color: [f32; 3], blur: f32) -> BTreeMap<String, ParamValue> {
    BTreeMap::from([
        ("offset".to_string(), ParamValue::Vec2(offset)),
        ("scale".to_string(), ParamValue::Float(scale)),
        ("color".to_string(), ParamValue::Color(color)),
        ("blur".to_string(), ParamValue::Float(blur)),
    ])
}

/// Wrap entries into the single `aberrations` List param the CA filter expects.
fn ca_params(entries: Vec<BTreeMap<String, ParamValue>>) -> Vec<ParamValue> {
    vec![ParamValue::List(entries)]
}

/// A single white entry offset 4 px right shifts the pixels: output at `x`
/// samples the source at `x + 4` (integer offset → exact bilinear tap).
#[test]
fn single_offset_aberration_shifts_pixels() {
    let (w, h) = (16u32, 8u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    let params = ca_params(vec![entry([4.0, 0.0], 1.0, [1.0, 1.0, 1.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    // Interior pixel: after(x) ≈ before(x+4), within a small bilinear tolerance.
    for (x, y) in [(1u32, 2u32), (5, 5), (9, 1)] {
        let a = px(&after, w, x, y);
        let b = px(&before, w, x + 4, y);
        for c in 0..3 {
            assert!(
                (a[c] as i32 - b[c] as i32).abs() <= 2,
                "shifted pixel ({x},{y}) ch{c}: after {a:?} vs before+4 {b:?}"
            );
        }
    }
    assert_ne!(after, before, "an offset aberration must change pixels");
}

/// R/G/B entries with zero offset, unit scale, no blur are a bit-exact
/// passthrough — each channel is sourced by exactly one entry and `inv_sum` is
/// 1, so the ε-normalization introduces no drift.
#[test]
fn identity_params_preserve_pixels_exactly() {
    let (w, h) = (12u32, 9u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    let params = ca_params(vec![
        entry([0.0, 0.0], 1.0, [1.0, 0.0, 0.0], 0.0),
        entry([0.0, 0.0], 1.0, [0.0, 1.0, 0.0], 0.0),
        entry([0.0, 0.0], 1.0, [0.0, 0.0, 1.0], 0.0),
    ]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "identity R/G/B aberration must be bit-exact"
    );
}

/// Radial scale pivots on the texture center: the exact center pixel is
/// unchanged while off-center pixels move.
#[test]
fn radial_scale_moves_edges_not_center() {
    let (w, h) = (15u32, 15u32); // odd → a true center pixel at (7,7), uv 0.5
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    let params = ca_params(vec![entry([0.0, 0.0], 1.1, [1.0, 1.0, 1.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    // Center pixel maps to itself under the radial scale.
    assert_eq!(
        px(&after, w, 7, 7),
        px(&before, w, 7, 7),
        "the center pixel is the scale pivot — unchanged"
    );
    // A far-from-center pixel is resampled and differs.
    assert_ne!(
        px(&after, w, 1, 1),
        px(&before, w, 1, 1),
        "an off-center pixel must move under radial scale"
    );
}

/// A rect selection clips the aberration to the selected region.
#[test]
fn selection_clips_aberration() {
    let (w, h) = (16u32, 16u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    e.select_rect(4.0, 4.0, 6.0, 6.0, SelectionMode::Replace, false, 0.0);
    let params = ca_params(vec![entry([3.0, 0.0], 1.0, [1.0, 1.0, 1.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    // Inside the selection: changed by the offset.
    assert_ne!(
        px(&after, w, 6, 6),
        px(&before, w, 6, 6),
        "a selected pixel must be aberrated"
    );
    // Outside the selection: untouched.
    assert_eq!(
        px(&after, w, 0, 0),
        px(&before, w, 0, 0),
        "a pixel outside the selection must be untouched"
    );
    assert_eq!(px(&after, w, 14, 14), px(&before, w, 14, 14));
}

/// Veil smoke test: the CA veil, driven through its own GPU path (registry →
/// `create_cache` → `encode`), produces non-identity output — a white entry
/// offset 4 px right shifts the whole-canvas result.
#[test]
fn veil_produces_non_identity_output() {
    let (device, queue) = test_device();
    let (w, h) = (16u32, 16u32);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pattern = distinct_rgba(w, h);

    let (_tex0, view0) = create_test_texture(&device, &queue, w, h, &pattern);
    let (_tex1, view1) = create_test_texture(&device, &queue, w, h, &[]);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let mut registry = VeilRegistry::new();
    let params = ca_params(vec![entry([4.0, 0.0], 1.0, [1.0, 1.0, 1.0], 0.0)]);
    let veil = registry.create_veil("chromatic_aberration", &params, &device, format);
    let cache = veil.create_cache(&device, &queue, &[view0, view1], &sampler, w, h);

    let (dst, dst_view) = create_test_texture(&device, &queue, w, h, &[]);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ca-veil-test"),
    });
    veil.encode(&mut encoder, &cache, 0, &dst_view);
    queue.submit(std::iter::once(encoder.finish()));

    let out = readback_texture(&device, &queue, &dst, format, w, h);
    assert_ne!(
        out, pattern,
        "CA veil with an offset must change the canvas"
    );
    // Interior pixel: out(x) ≈ pattern(x+4).
    let a = px(&out, w, 2, 2);
    let b = px(&pattern, w, 6, 2);
    for c in 0..3 {
        assert!(
            (a[c] as i32 - b[c] as i32).abs() <= 2,
            "veil shift ch{c}: out {a:?} vs pattern+4 {b:?}"
        );
    }
}

/// Undo restores the pristine pixels (the `GpuRegionAction` path).
#[test]
fn undo_restores_pristine_pixels() {
    let (w, h) = (12u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    let params = ca_params(vec![entry([5.0, 2.0], 1.02, [1.0, 0.2, 0.2], 1.5)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    assert_ne!(
        e.test_readback_layer(layer),
        before,
        "filter changed pixels"
    );

    e.undo();
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "undo restores the pristine pixels"
    );
}
