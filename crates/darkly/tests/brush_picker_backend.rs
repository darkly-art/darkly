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

#[test]
fn brush_dab_thumbnail_first_call_kicks_bake_then_returns_png() {
    let mut engine = fresh_engine();

    // First call schedules a bake and returns empty; second call is a
    // duplicate-suppress no-op until the readback lands.
    let first = engine.brush_dab_thumbnail("Soft Round");
    assert!(
        first.is_empty(),
        "first call returns empty while the dab bake is in flight"
    );
    let second = engine.brush_dab_thumbnail("Soft Round");
    assert!(
        second.is_empty(),
        "back-to-back call before flush stays empty (no double-queue)"
    );

    engine.test_flush_readbacks();
    let third = engine.brush_dab_thumbnail("Soft Round");
    assert!(
        !third.is_empty(),
        "after flush the dab cache holds a baked PNG"
    );
    assert_eq!(
        &third[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "bytes start with the PNG signature"
    );
}

#[test]
fn theme_change_invalidates_dab_thumbnail() {
    let mut engine = fresh_engine();

    let _ = engine.brush_dab_thumbnail("Soft Round");
    engine.test_flush_readbacks();
    let dark = engine.brush_dab_thumbnail("Soft Round");
    assert!(!dark.is_empty(), "dark-theme dab bake produced bytes");

    engine.set_preview_theme([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
    let pending = engine.brush_dab_thumbnail("Soft Round");
    assert!(
        pending.is_empty(),
        "first call after theme swap returns empty while the re-bake is in flight"
    );

    engine.test_flush_readbacks();
    let light = engine.brush_dab_thumbnail("Soft Round");
    assert!(!light.is_empty(), "rebake produces fresh bytes");
    assert_ne!(
        light, dark,
        "inverted theme should produce different dab PNG bytes"
    );
}

/// Decode a baked dab thumbnail and report what fraction of pixels are
/// non-bg (stroke content). The default preview theme bg is roughly
/// (20, 20, 20); anything brighter than mid-grey can only have come
/// from the white stroke fg.
fn decoded_dab_content_ratio(png: &[u8]) -> f64 {
    let img = image::load_from_memory(png).expect("valid PNG");
    let rgba = img.to_rgba8();
    let bright = rgba.pixels().filter(|p| p.0[0] > 128).count();
    let total = (rgba.width() * rgba.height()) as usize;
    bright as f64 / total as f64
}

/// Regression: brushes with deliberately small `size` ports (Airbrush
/// holds `stamp.size = 0.15`) used to render as a tiny dot in the
/// middle of the dab thumbnail because the canvas was sized for
/// pressure-driven brushes that go full-bleed at pressure=1. The bake
/// path now bbox-crops the rendered dab and rescales, so the picker
/// tile shows a recognizable shape regardless of the brush's size port
/// setting.
#[test]
fn small_size_brush_dab_thumbnail_is_framed() {
    let mut engine = fresh_engine();
    let _ = engine.brush_dab_thumbnail("Airbrush");
    engine.test_flush_readbacks();
    let png = engine.brush_dab_thumbnail("Airbrush");
    let ratio = decoded_dab_content_ratio(&png);
    assert!(
        ratio > 0.10,
        "Airbrush dab should fill at least 10% of the framed thumbnail; got {:.1}%",
        ratio * 100.0
    );
}

/// Scatter Brush displaces every dab by ±dab_size in x/y via a scatter
/// node. With a single-sample dab path that was enough to push the
/// dab off the small render canvas entirely for some seeds. The bake
/// must produce visible content regardless — render headroom + bbox
/// crop keeps the dab visible no matter where the scatter lands.
#[test]
fn scatter_brush_dab_thumbnail_has_visible_content() {
    let mut engine = fresh_engine();
    let _ = engine.brush_dab_thumbnail("Scatter Brush");
    engine.test_flush_readbacks();
    let png = engine.brush_dab_thumbnail("Scatter Brush");
    let ratio = decoded_dab_content_ratio(&png);
    assert!(
        ratio > 0.05,
        "Scatter Brush dab should produce visible content somewhere in the framed thumbnail; got {:.1}%",
        ratio * 100.0
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
    assert!(
        first.is_empty(),
        "cache miss returns an empty Vec — frontends use that as 'no fresh \
         bytes' so the previous render stays on screen instead of flashing \
         transparent. Got {} bytes.",
        first.len(),
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
    assert!(
        after_invalidate_first.is_empty(),
        "theme change drops the cache; next call returns empty until the \
         rebake lands. Got {} bytes.",
        after_invalidate_first.len(),
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
fn size_scrub_does_not_change_active_dab_pixels() {
    // The dab thumbnail represents brush identity (shape, texture,
    // dynamics) — scrubbing the brush bar's user-facing size should leave
    // the icon visually unchanged. Verified end-to-end: render, scrub
    // size, render again, compare bytes.
    //
    // Also locks in the topology-version contract the brush-bar UI relies
    // on: a scrub must not advance `brush_topology_version`. The frontend
    // uses that counter to decide whether the active preset name still
    // applies — a false bump would flip "Soft Round" → "Custom" on every
    // size drag. Regression for that bug lives here, against the same
    // engine, to avoid creating an extra wgpu device in parallel.
    let mut engine = fresh_engine();
    let (w, h) = (32u32, 32u32);

    let _ = engine.brush_active_dab_preview(w, h);
    engine.test_flush_readbacks();
    let before = engine.brush_active_dab_preview(w, h);
    assert!(before.iter().any(|&b| b != 0));

    // Find the stamp's exposed `size` port and adjust it via the same
    // entry point the brush bar / shift+drag scrub uses.
    let size = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.port_name == "size")
        .expect("default brush exposes a `size` port");
    let topo_before_scrub = engine.brush_topology_version();
    engine
        .brush_set_exposed_port(size.node_id, "size", 250.0)
        .expect("scrub set");
    assert_eq!(
        engine.brush_topology_version(),
        topo_before_scrub,
        "exposed-port scrub must not advance the topology version — \
         the frontend uses this to keep the active preset name across scrubs"
    );

    // Drain any in-flight readback queued by the scrub. If the cache
    // were keyed off graph_version it would have invalidated and a
    // rebake would run; under the topology-version split it doesn't.
    engine.test_flush_readbacks();
    let after = engine.brush_active_dab_preview(w, h);
    assert_eq!(
        after, before,
        "scrubbing the user-facing size port must not change the dab thumbnail bytes"
    );

    // Conversely, a structural change MUST advance the topology version,
    // so the frontend correctly clears the preset name. Toggle a port's
    // exposed flag — cheaper than brush_load and avoids extra GPU work
    // (no compile_active call), but still classified as topology.
    let topo_before_toggle = engine.brush_topology_version();
    engine
        .brush_graph_set_port_exposed(size.node_id, "size", false)
        .expect("toggle exposed");
    assert_ne!(
        engine.brush_topology_version(),
        topo_before_toggle,
        "set_port_exposed is a structural change and must advance the topology version"
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
