//! Backend coverage for the v1 brush picker.
//!
//! Exercises the lazy thumbnail bake and active-dab preview cache through
//! a real `DarklyEngine` with a headless GPU context. Each behaviour the
//! frontend depends on — first call returns empty, post-flush returns
//! valid bytes, version + theme invalidation drop the cache — is asserted
//! end-to-end via the test-only `test_flush_readbacks` helper.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 1024, 768)
}

#[test]
fn brush_thumbnail_first_call_kicks_bake_then_returns_png() {
    let mut engine = fresh_engine();

    // First call returns empty bytes — the bake was scheduled, not run.
    let first = engine.brush_thumbnail("Soft Round");
    assert!(
        first.is_empty(),
        "first call should return empty bytes while the bake is in flight"
    );

    // Calling again before the readback completes returns empty too —
    // we don't queue a second bake on top of an in-flight one.
    let second = engine.brush_thumbnail("Soft Round");
    assert!(
        second.is_empty(),
        "second call before flush should still be empty"
    );

    // Flush the pending readback and confirm the library entry now
    // carries a PNG-encoded thumbnail.
    engine.test_flush_readbacks();
    let third = engine.brush_thumbnail("Soft Round");
    assert!(
        !third.is_empty(),
        "after flush the library entry should hold the baked PNG"
    );
    assert_eq!(
        &third[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "bytes start with the PNG signature"
    );
}

#[test]
fn theme_change_invalidates_brush_thumbnail() {
    let mut engine = fresh_engine();

    // Bake under the dark default theme.
    let _ = engine.brush_thumbnail("Soft Round");
    engine.test_flush_readbacks();
    let dark_png = engine.brush_thumbnail("Soft Round");
    assert!(!dark_png.is_empty(), "dark-theme bake produced bytes");
    assert_eq!(
        &dark_png[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
    );

    // Swap to the inverted (light) palette. The library entry should
    // drop its baked PNG so the next picker fetch kicks off a re-bake.
    engine.set_preview_theme([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);

    let pending = engine.brush_thumbnail("Soft Round");
    assert!(
        pending.is_empty(),
        "first call after theme swap returns empty while the re-bake is in flight"
    );

    engine.test_flush_readbacks();
    let light_png = engine.brush_thumbnail("Soft Round");
    assert!(
        !light_png.is_empty(),
        "post-flush bytes are present under the new theme"
    );
    assert_ne!(
        light_png, dark_png,
        "inverted theme should produce different PNG bytes"
    );
}

/// Regression: image-based brushes (Calligraphy, Textured Ink, Pencil,
/// Charcoal, Canvas Brush) embed PNG tip resources. The picker's lazy
/// thumbnail bake fired against the active brush's `resource_handles`,
/// so any brush *not currently loaded* lacked its tip texture and baked
/// to the theme bg only — every image-based tile in the picker grid
/// looked blank until the user clicked it.
#[test]
fn image_based_brush_thumbnail_renders_with_resource() {
    let mut engine = fresh_engine();

    // Calligraphy is not the active brush — the active graph is the
    // built-in default (a procedural circle stamp). The picker still
    // has to bake Calligraphy's thumbnail using its own embedded
    // calligraphy.png tip.
    let _ = engine.brush_thumbnail("Calligraphy");
    engine.test_flush_readbacks();
    let png = engine.brush_thumbnail("Calligraphy");
    assert!(!png.is_empty(), "Calligraphy bake should produce bytes");

    // Decode the PNG and look for stroke pixels. The default preview
    // theme bg is ~#141414 (0.08 linear); anything brighter than mid-
    // grey can only come from the white stroke fg. If the tip texture
    // wasn't uploaded, the bake produces a uniform bg with no stroke.
    let img = image::load_from_memory(&png).expect("valid PNG");
    let rgba = img.to_rgba8();
    let bright_pixels = rgba.pixels().filter(|p| p.0[0] > 128).count();
    assert!(
        bright_pixels > 0,
        "Calligraphy bake should include stroke pixels — got {bright_pixels} bright pixels, \
         which means the tip texture wasn't picked up by the renderer"
    );
}

#[test]
fn brush_thumbnail_unknown_name_returns_empty() {
    let mut engine = fresh_engine();
    let bytes = engine.brush_thumbnail("Definitely Not A Real Brush");
    assert!(
        bytes.is_empty(),
        "unknown brush names return empty without queueing anything"
    );
    // No readbacks were queued, so flush is a no-op.
    engine.test_flush_readbacks();
    let bytes = engine.brush_thumbnail("Definitely Not A Real Brush");
    assert!(bytes.is_empty(), "still empty after flush");
}

#[test]
fn active_dab_preview_first_call_empty_then_present_after_flush() {
    let mut engine = fresh_engine();

    let (w, h) = (40u32, 40u32);
    let first = engine.brush_active_dab_preview(w, h);
    assert_eq!(
        first,
        vec![0u8; (w * h * 4) as usize],
        "first call returns a zero-filled RGBA buffer of the requested size"
    );

    engine.test_flush_readbacks();
    let second = engine.brush_active_dab_preview(w, h);
    assert_eq!(
        second.len(),
        (w * h * 4) as usize,
        "post-flush bytes match the requested dimensions"
    );
    assert!(
        second.iter().any(|&b| b != 0),
        "rendered dab should produce some non-zero pixels"
    );
}

#[test]
fn active_dab_preview_cached_across_calls() {
    let mut engine = fresh_engine();

    let (w, h) = (40u32, 40u32);
    let _ = engine.brush_active_dab_preview(w, h);
    engine.test_flush_readbacks();
    let cached_a = engine.brush_active_dab_preview(w, h);
    let cached_b = engine.brush_active_dab_preview(w, h);
    assert_eq!(
        cached_a, cached_b,
        "back-to-back calls without invalidation return the same cached pixels"
    );
}

#[test]
fn theme_change_invalidates_active_dab_preview() {
    let mut engine = fresh_engine();

    let (w, h) = (40u32, 40u32);
    let _ = engine.brush_active_dab_preview(w, h);
    engine.test_flush_readbacks();
    let before = engine.brush_active_dab_preview(w, h);
    assert!(before.iter().any(|&b| b != 0), "baseline has rendered dab");

    // Swap to a contrasting palette — invalidation drops the cache so
    // the next call has to re-bake. The shape of the buffer doesn't
    // change, but the bg pixels (everywhere outside the dab) shift to
    // the new background colour, so byte-equality must fail.
    engine.set_preview_theme([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
    let after_invalidate_first = engine.brush_active_dab_preview(w, h);
    assert_eq!(
        after_invalidate_first,
        vec![0u8; (w * h * 4) as usize],
        "theme change drops the cache; next call returns zeros until the rebake lands"
    );

    engine.test_flush_readbacks();
    let after = engine.brush_active_dab_preview(w, h);
    assert!(
        after.iter().any(|&b| b != 0),
        "rebake produces fresh pixels"
    );
    assert_ne!(
        after, before,
        "different theme should yield different pixels"
    );
}

#[test]
fn graph_change_triggers_active_dab_rebake() {
    let mut engine = fresh_engine();

    let (w, h) = (40u32, 40u32);
    let _ = engine.brush_active_dab_preview(w, h);
    engine.test_flush_readbacks();
    let before = engine.brush_active_dab_preview(w, h);
    assert!(before.iter().any(|&b| b != 0));

    // Loading a different brush replaces the active graph, which bumps
    // the graph version through `compile_active`. The version mismatch
    // queues a fresh render on the next call (the previous bytes are
    // returned as fallback to avoid a flash to zeros mid-swap).
    engine
        .brush_load("Hard Round")
        .expect("Hard Round is a built-in brush");
    let _stale_fallback = engine.brush_active_dab_preview(w, h);

    engine.test_flush_readbacks();
    let after = engine.brush_active_dab_preview(w, h);
    assert!(
        after.iter().any(|&b| b != 0),
        "rebake under the new brush produces fresh pixels"
    );
    assert_ne!(
        after, before,
        "swapping Soft Round for Hard Round should produce different dab pixels"
    );
}
