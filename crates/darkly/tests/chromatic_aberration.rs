//! Destructive chromatic-aberration integration tests — the CA filter over the
//! shared `filter_node_region` substrate, exercising the new `SrcSampling::
//! Bilinear` source mode and the List/Color/Vec2 param kinds end to end.
//!
//! Run with: `cargo test -p darkly --test chromatic_aberration --features testing -- --test-threads=1`

use std::collections::BTreeMap;

use darkly::document::SelectionMode;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::effect::EffectRegistry;
use darkly::gpu::params::ParamValue;
use darkly::gpu::test_utils::*;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// A `w`×`h` flat opaque RGBA buffer of a single color.
fn solid_rgba(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for px in v.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&color);
    }
    v
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
/// passthrough — every displaced sample equals the base, so all content deltas
/// are zero and no entry contributes anything.
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

    let mut registry = EffectRegistry::new();
    let params = ca_params(vec![entry([4.0, 0.0], 1.0, [1.0, 1.0, 1.0], 0.0)]);
    let mut veil = registry
        .instance("chromatic_aberration", &params, &device, format)
        .expect("registered effect");
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

/// A single non-primary (orange) entry over a flat fill leaves the interior
/// bit-exact: with all deltas zero away from the border, the hue-rotation model
/// reduces to passthrough (no whole-image recolor). Fails under the old
/// `inv_sum`-normalized reconstruction, which paints the canvas orange.
#[test]
fn flat_image_single_orange_entry_is_interior_passthrough() {
    let (w, h) = (16u32, 12u32);
    let mut e = test_engine(w, h);
    let fill = solid_rgba(w, h, [180, 90, 40, 255]);
    let layer = e.paste_image(w, h, &fill, 0, 0, None);
    let before = e.test_readback_layer(layer);

    // Orange color, offset (4,3): interior samples land in-bounds on the flat
    // fill, so every content delta is zero.
    let params = ca_params(vec![entry([4.0, 3.0], 1.0, [1.0, 0.5, 0.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    // Interior: farther than the (4,3) offset from the right/bottom border, so
    // the shifted tap stays on the flat fill.
    for y in 0..(h - 3) {
        for x in 0..(w - 4) {
            assert_eq!(
                px(&after, w, x, y),
                px(&before, w, x, y),
                "interior pixel ({x},{y}) must be untouched by a flat-fill aberration"
            );
        }
    }
}

/// A single red `(1,0,0)` entry shifts only the red channel: green/blue stay
/// bit-exact in the interior, red is sourced from the offset position. Fails
/// under the old model, which zeroes green and blue (they carry no entry weight).
#[test]
fn red_entry_shifts_only_red_channel() {
    let (w, h) = (16u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    let params = ca_params(vec![entry([4.0, 3.0], 1.0, [1.0, 0.0, 0.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    let mut red_moved = false;
    for y in 0..(h - 3) {
        for x in 0..(w - 4) {
            let a = px(&after, w, x, y);
            let b = px(&before, w, x, y);
            // Green and blue untouched everywhere in the interior.
            assert_eq!(a[1], b[1], "green must be untouched at ({x},{y})");
            assert_eq!(a[2], b[2], "blue must be untouched at ({x},{y})");
            // Red is sourced from the offset (4,3) position.
            let shifted = px(&before, w, x + 4, y + 3);
            assert!(
                (a[0] as i32 - shifted[0] as i32).abs() <= 2,
                "red at ({x},{y}) must match input at the offset: after {a:?} vs +offset {shifted:?}"
            );
            if a[0] != b[0] {
                red_moved = true;
            }
        }
    }
    assert!(
        red_moved,
        "the red channel must differ from the input somewhere"
    );
}

/// An arbitrary-color entry at identity (offset 0, scale 1, blur 0) is a
/// bit-exact passthrough for the whole image. Fails under the old model, which
/// recolors every pixel toward the entry color.
#[test]
fn identity_arbitrary_color_is_exact_passthrough() {
    let (w, h) = (12u32, 9u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    let params = ca_params(vec![entry([0.0, 0.0], 1.0, [0.3, 0.8, 0.2], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    assert_eq!(
        e.test_readback_layer(layer),
        before,
        "an identity aberration of any color must be bit-exact"
    );
}

/// A chromatic entry over a fully-opaque interior must not erode alpha: where
/// the displaced tap stays in-bounds there is no coverage change, so a pure hue
/// shift leaves every pixel opaque. Regression for the bug where the chromatic
/// alpha term projected the *color* delta (not the coverage delta), leaving
/// semi-transparent patches inside solid areas. Fails on the pre-fix shader,
/// which drops alpha wherever the shifted red channel decreases.
#[test]
fn chromatic_entry_preserves_opaque_interior_alpha() {
    let (w, h) = (16u32, 8u32);
    let mut e = test_engine(w, h);
    // Opaque image, red descending across x (constant low green/blue). A +x red
    // shift then samples a *lower* red everywhere → a negative color delta.
    let mut img = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            img[i] = (255 - x * 15) as u8;
            img[i + 1] = 40;
            img[i + 2] = 40;
            img[i + 3] = 255;
        }
    }
    let layer = e.paste_image(w, h, &img, 0, 0, None);

    // Pure red entry, offset +3 x, no blur: interior taps (x + 3 < w) stay on the
    // opaque fill, so alpha must remain 255 for every one of them.
    let params = ca_params(vec![entry([3.0, 0.0], 1.0, [1.0, 0.0, 0.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    for y in 0..h {
        for x in 0..(w - 3) {
            assert_eq!(
                px(&after, w, x, y)[3],
                255,
                "opaque interior pixel ({x},{y}) must stay fully opaque"
            );
        }
    }
}

/// A `w`×`h` buffer split down the middle: `left` for `x < w/2`, `right`
/// otherwise. Used to build opaque/transparent boundaries.
fn half_split(w: u32, h: u32, left: [u8; 4], right: [u8; 4]) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let c = if x < w / 2 { left } else { right };
            v[i..i + 4].copy_from_slice(&c);
        }
    }
    v
}

/// A hue-120° `(0,1,0)` entry is exactly a green-channel shift: red and blue
/// stay bit-exact in the interior, green is sourced from the offset. Pins the
/// exact-channel-permutation property of the rotation axis at ±120°.
#[test]
fn hue_120_entry_is_exact_green_channel_shift() {
    let (w, h) = (16u32, 12u32);
    let mut e = test_engine(w, h);
    let layer = e.paste_image(w, h, &distinct_rgba(w, h), 0, 0, None);
    let before = e.test_readback_layer(layer);

    let params = ca_params(vec![entry([4.0, 3.0], 1.0, [0.0, 1.0, 0.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    let mut green_moved = false;
    for y in 0..(h - 3) {
        for x in 0..(w - 4) {
            let a = px(&after, w, x, y);
            let b = px(&before, w, x, y);
            assert_eq!(a[0], b[0], "red must be untouched at ({x},{y})");
            assert_eq!(a[2], b[2], "blue must be untouched at ({x},{y})");
            let shifted = px(&before, w, x + 4, y + 3);
            assert!(
                (a[1] as i32 - shifted[1] as i32).abs() <= 2,
                "green at ({x},{y}) must match input at the offset"
            );
            if a[1] != b[1] {
                green_moved = true;
            }
        }
    }
    assert!(
        green_moved,
        "the green channel must differ from the input somewhere"
    );
}

/// When a channel is shifted off an alpha edge, the surviving content stays
/// visible: shifting the red out of opaque yellow (across a transparent
/// boundary) leaves an *opaque* green — the representability floor keeps alpha
/// from collapsing while a channel remains.
#[test]
fn shifted_away_channel_keeps_remaining_content_visible() {
    let (w, h) = (16u32, 8u32);
    let mut e = test_engine(w, h);
    // Left half opaque yellow, right half transparent.
    let img = half_split(w, h, [255, 255, 0, 255], [0, 0, 0, 0]);
    let layer = e.paste_image(w, h, &img, 0, 0, None);

    // Red entry, offset +4 x: a yellow pixel near the boundary samples its red
    // from the transparent region → the red departs, green remains.
    let params = ca_params(vec![entry([4.0, 0.0], 1.0, [1.0, 0.0, 0.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    // x=6 samples x=10 (transparent): opaque yellow → opaque green.
    assert_eq!(
        px(&after, w, 6, 4),
        [0, 255, 0, 255],
        "yellow losing its red across a transparent edge must stay opaque green"
    );
}

/// Content shifted onto transparency becomes visible with the alpha of its
/// source: a red region pulled into a transparent region by a red entry appears
/// as an opaque red ghost.
#[test]
fn shifted_content_becomes_visible_over_transparency() {
    let (w, h) = (16u32, 8u32);
    let mut e = test_engine(w, h);
    // Left half transparent, right half opaque red.
    let img = half_split(w, h, [0, 0, 0, 0], [255, 0, 0, 255]);
    let layer = e.paste_image(w, h, &img, 0, 0, None);

    // Red entry, offset +4 x: a transparent pixel at x=4 samples x=8 (opaque
    // red) → its red content is pulled in over the transparency.
    let params = ca_params(vec![entry([4.0, 0.0], 1.0, [1.0, 0.0, 0.0], 0.0)]);
    assert!(e.apply_filter_typed(layer, "chromatic_aberration", params));
    let after = e.test_readback_layer(layer);

    // x=4 samples x=8 (opaque red): transparent → opaque red ghost.
    assert_eq!(
        px(&after, w, 4, 4),
        [255, 0, 0, 255],
        "red content shifted onto transparency must appear as an opaque red ghost"
    );
    // A transparent pixel whose sample stays transparent is untouched.
    assert_eq!(
        px(&after, w, 0, 4),
        [0, 0, 0, 0],
        "transparency sampling transparency stays transparent"
    );
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
