//! Native-only integration tests for the full-stroke brush editor preview.
//!
//! Asserts the stroke engine runs end-to-end against the preview's own
//! scratch target and produces non-empty RGBA pixels where the S-curve was
//! drawn. Uses the blocking `test_utils::readback_texture` helper — native
//! only; the wasm path does async readback via the ReadbackScheduler.

use std::collections::HashMap;

use darkly::brush::{
    dab_pool::DabTexturePool,
    default_graph,
    pipelines::BrushPipelines,
    preview_renderer::{synthesize_preview_stroke, BrushPreviewRenderer},
};
use darkly::gpu::test_utils::{readback_texture, test_device};

#[test]
fn renders_s_curve_over_black_background() {
    let (device, queue) = test_device();
    let mut dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout());
    let resources: HashMap<_, _> = HashMap::new();
    let mut renderer = BrushPreviewRenderer::new();
    let graph = default_graph();

    let width: u32 = 320;
    let height: u32 = 120;
    let path = synthesize_preview_stroke(width as f32, height as f32, 30, 0.0);

    let fg = [1.0, 1.0, 1.0, 1.0]; // white stroke
    let bg = [0.0, 0.0, 0.0, 1.0]; // black background

    let texture = renderer
        .render_stroke(
            &device,
            &queue,
            &mut dab_pool,
            &pipelines,
            &resources,
            &graph,
            &path,
            fg,
            bg,
            width,
            height,
        )
        .expect("render_stroke should return a texture for the default graph");

    let pixels = readback_texture(
        &device,
        &queue,
        texture,
        wgpu::TextureFormat::Rgba8Unorm,
        width,
        height,
    );

    assert_eq!(pixels.len(), (width * height * 4) as usize);

    // Pixel at (x, y), RGBA.
    let get = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };

    // A corner not crossed by the stroke should still be the solid bg.
    // Top-right corner falls outside the S-curve envelope.
    let corner = get(width - 2, 1);
    assert_eq!(
        corner[3], 255,
        "bg alpha should remain opaque away from the stroke"
    );
    assert!(
        corner[0] < 40 && corner[1] < 40 && corner[2] < 40,
        "bg corner should stay near-black, got {:?}",
        corner
    );

    // The S-curve passes through the geometric center near peak pressure.
    // At least one nearby sample should be brighter than the background.
    let mut any_bright = false;
    for dy in -10i32..=10 {
        for dx in -10i32..=10 {
            let x = (width as i32 / 2 + dx) as u32;
            let y = (height as i32 / 2 + dy) as u32;
            let px = get(x, y);
            if px[0] > 64 || px[1] > 64 || px[2] > 64 {
                any_bright = true;
            }
        }
    }
    assert!(
        any_bright,
        "expected bright pixels near the center along the S-curve"
    );

    // Deliberately no wall-clock assertion here. Render time is dominated
    // by the GPU backend: ~5-20 ms on native Vulkan/Metal, several
    // hundred ms on CI's software fallback (lavapipe). Any bound loose
    // enough for CI catches only cartoonish regressions; any bound tight
    // enough to be meaningful flakes on CI. Perf tracking for this path
    // belongs in a dedicated bench on hardware, not here.
}

#[test]
fn renderer_reuses_target_across_renders_of_same_size() {
    let (device, queue) = test_device();
    let mut dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout());
    let resources: HashMap<_, _> = HashMap::new();
    let mut renderer = BrushPreviewRenderer::new();
    let graph = default_graph();
    let path = synthesize_preview_stroke(320.0, 120.0, 20, 0.0);

    assert!(renderer.current_size().is_none());

    let _ = renderer.render_stroke(
        &device,
        &queue,
        &mut dab_pool,
        &pipelines,
        &resources,
        &graph,
        &path,
        [1.0, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        320,
        120,
    );
    assert_eq!(renderer.current_size(), Some((320, 120)));
    let first_ptr = renderer.current_texture().map(|t| t as *const _);

    let _ = renderer.render_stroke(
        &device,
        &queue,
        &mut dab_pool,
        &pipelines,
        &resources,
        &graph,
        &path,
        [1.0, 0.0, 0.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
        320,
        120,
    );
    let second_ptr = renderer.current_texture().map(|t| t as *const _);

    // Same size → same underlying texture.
    assert_eq!(first_ptr, second_ptr);
}

#[test]
fn engine_brush_editor_preview_caches_after_readback() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    let width: u32 = 320;
    let height: u32 = 120;

    // First call: cache empty, kicks off a readback, returns an empty Vec
    // — the frontend uses that as a "no fresh bytes" signal so it
    // preserves whatever was last shown rather than flashing transparent.
    let first = engine.brush_editor_preview(width, height);
    assert!(
        first.is_empty(),
        "cache miss should return empty Vec, got {} bytes",
        first.len()
    );

    // Flush the in-flight readback (native-only helper; wasm relies on the
    // event loop polling the ReadbackScheduler via the render loop).
    engine.test_flush_readbacks();

    // Second call: cache now populated with the real pixels. A non-trivial
    // fraction of pixels should be non-zero (the stroke deposited ink).
    let second = engine.brush_editor_preview(width, height);
    assert_eq!(second.len(), (width * height * 4) as usize);
    let non_zero_pixels = second
        .chunks_exact(4)
        .filter(|px| px[0] > 0 || px[1] > 0 || px[2] > 0)
        .count();
    assert!(
        non_zero_pixels > 100,
        "expected non-trivial stroke coverage in cached preview, got {non_zero_pixels} non-zero pixels"
    );
}

#[test]
fn engine_brush_editor_preview_skips_unchanged_graph() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Prime the cache.
    let _ = engine.brush_editor_preview(320, 120);
    engine.test_flush_readbacks();
    let first = engine.brush_editor_preview(320, 120);

    // Without touching the graph, a second call returns the same cache
    // and does not queue another readback.
    let second = engine.brush_editor_preview(320, 120);
    assert_eq!(first, second);
}

#[test]
fn set_preview_theme_invalidates_cache() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Prime the cache with the default (dark) theme: white on dark.
    engine.set_preview_theme([1.0, 1.0, 1.0, 1.0], [0.02, 0.02, 0.02, 1.0]);
    let _ = engine.brush_editor_preview(320, 120);
    engine.test_flush_readbacks();
    let dark_pixels = engine.brush_editor_preview(320, 120);

    // Switch to the light theme: black on light. Cache should invalidate
    // and the next readback should produce distinctly different pixels.
    engine.set_preview_theme([0.0, 0.0, 0.0, 1.0], [0.9, 0.9, 0.9, 1.0]);
    let after_change = engine.brush_editor_preview(320, 120);
    // Pre-readback call returns the zero placeholder (cache was invalidated).
    assert!(after_change.iter().all(|&b| b == 0));

    engine.test_flush_readbacks();
    let light_pixels = engine.brush_editor_preview(320, 120);

    assert_ne!(
        dark_pixels, light_pixels,
        "theme change must produce new preview pixels"
    );
    // Sanity-check: the light-theme preview has bright bg pixels.
    let mut bright_bg = 0;
    for chunk in light_pixels.chunks_exact(4) {
        if chunk[0] > 200 && chunk[1] > 200 && chunk[2] > 200 {
            bright_bg += 1;
        }
    }
    assert!(
        bright_bg > 1000,
        "light theme preview should have many bright bg pixels, got {bright_bg}"
    );
}

#[test]
fn brush_save_bakes_thumbnail_asynchronously() {
    use darkly::brush::bundle::Brush;
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Save a brush — kicks off an async thumbnail readback against the
    // engine's library copy.
    engine.brush_save("TestBrush", "basic").unwrap();

    // Before the readback lands, the library entry has no thumbnail.
    let exported_before = engine.brush_export("TestBrush").expect("brush exported");
    let bundle_before = Brush::from_bytes(&exported_before).unwrap();
    assert!(
        bundle_before.thumbnail_png.is_none(),
        "thumbnail should be absent before readback completes"
    );

    // Flush the pending readback; the completion handler writes the PNG
    // back onto the library entry.
    engine.test_flush_readbacks();

    let exported_after = engine.brush_export("TestBrush").unwrap();
    let bundle_after = Brush::from_bytes(&exported_after).unwrap();
    let png = bundle_after
        .thumbnail_png
        .expect("thumbnail present after readback");
    // Valid PNG — starts with the PNG magic signature.
    assert_eq!(
        &png[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "library entry now carries a PNG-encoded thumbnail"
    );
}

/// Regression: Hard Round (no `pressure → size_input` wire) paints every
/// dab at full size, including the endpoints. The endpoint dabs must not
/// be clipped against the cache border — the leftmost and rightmost
/// columns of the framed preview must contain only background pixels.
///
/// This is the user-visible bug: with the previous size-aware inset
/// hack, the path was shrunk so endpoints landed inside the canvas,
/// but the inset clamped to half the canvas at any non-trivial size and
/// the path degenerated. Without an inset, endpoints sit on the canvas
/// edge and the framer can't recover the clipped half of the dab.
#[test]
fn hard_round_endpoint_dabs_not_clipped_against_cache_border() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Pin the theme so the bg pixel value is deterministic for the test
    // — black bg, white stroke.
    engine.set_preview_theme([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);

    // Hard Round is a built-in: circle tip, no pressure→size_input wire.
    engine
        .brush_load("Hard Round")
        .expect("Hard Round built-in");

    let width: u32 = 320;
    let height: u32 = 120;

    // Prime + flush + read.
    let _ = engine.brush_editor_preview(width, height);
    engine.test_flush_readbacks();
    let pixels = engine.brush_editor_preview(width, height);
    assert_eq!(pixels.len(), (width * height * 4) as usize);

    // bg is black; mark anything noticeably brighter as stroke.
    const TOLERANCE: u8 = 16;
    let is_stroke = |i: usize| -> bool {
        pixels[i] > TOLERANCE || pixels[i + 1] > TOLERANCE || pixels[i + 2] > TOLERANCE
    };

    // The leftmost and rightmost columns must be entirely background —
    // any stroke pixel there means an endpoint dab was clipped.
    let edge_band = 1u32;
    for x_band in [0..edge_band, (width - edge_band)..width] {
        for x in x_band {
            for y in 0..height {
                let i = ((y * width + x) * 4) as usize;
                assert!(
                    !is_stroke(i),
                    "Hard Round preview cuts off at the edge — column {x} y={y} \
                     has stroke pixel rgba={:?}",
                    [pixels[i], pixels[i + 1], pixels[i + 2]],
                );
            }
        }
    }
}

/// Regression: scrubbing a `pen_input.stabilize` setting must not
/// invalidate the editor-preview cache. The synthetic-stroke preview
/// always renders with `PassThrough`, so the rendered pixels can't
/// change in response to a user scrub. Bumping `brush_graph_version`
/// on these scrubs would trigger a wasted full-stroke re-render every
/// 100 ms while the user drags the slider (~1 GB/s of GPU work for no
/// visible effect).
///
/// The fix declares stabilize via `with_preview_value(0.0)` and routes
/// scrubs on any preview-irrelevant port through
/// `ChangeKind::PreviewIrrelevantScrub`, which skips the version bump.
/// Asserted against the public `brush_graph_version()` getter, with a
/// negative-control scrub (`stamp.rotation`, no `preview_value`) to
/// guard against the rule being over-broad.
#[test]
fn stabilize_scrub_does_not_bump_editor_preview_version() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Ink Pen exposes both `stabilize` (default 0.6) and `size` so we can
    // contrast a preview-irrelevant scrub against a preview-affecting
    // one in the same engine — avoids creating a second wgpu device.
    engine.brush_load("Ink Pen").expect("Ink Pen built-in");

    // Prime the editor preview cache and let the readback land so the
    // version counter is at its post-init steady state.
    let _ = engine.brush_editor_preview(320, 120);
    engine.test_flush_readbacks();
    let v_before_stabilize = engine.brush_graph_version();

    // Find the exposed `stabilize` port and scrub it through
    // `brush_set_exposed_port` — the same entry point the brush bar uses.
    let stabilize = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.port_name == "stabilize")
        .expect("Ink Pen exposes a `stabilize` port");
    engine
        .brush_set_exposed_port(stabilize.node_id, "stabilize", 90.0)
        .expect("scrub set");

    assert_eq!(
        engine.brush_graph_version(),
        v_before_stabilize,
        "stabilize is preview-irrelevant (PassThrough is hardcoded for \
         the synthetic stroke); scrubbing it must not bump \
         brush_graph_version — bumping invalidates the editor preview \
         cache and triggers a wasted full-stroke re-render."
    );

    // Negative control: scrubbing a port the preview *does* read must
    // still bump the version. `stamp.rotation` has no `preview_value`,
    // is read by the stamp shader, and (for Ink Pen) is unwired — the
    // perfect canary for "rule too broad". `brush_set_exposed_port`
    // doesn't gate on the `exposed` flag (only the listing API does),
    // so we reuse the stamp node id from the exposed `size` port.
    let size = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.port_name == "size")
        .expect("Ink Pen exposes a `size` port on the stamp node");
    let v_before_rotation = engine.brush_graph_version();
    engine
        .brush_set_exposed_port(size.node_id, "rotation", 0.5)
        .expect("scrub set");
    assert_ne!(
        engine.brush_graph_version(),
        v_before_rotation,
        "rotation has no preview_value → it affects the preview output \
         → its scrub must bump brush_graph_version. If this assertion \
         fails, the preview-irrelevant rule is over-broad and real \
         preview updates would also stall."
    );
}

#[test]
fn empty_path_returns_none() {
    let (device, queue) = test_device();
    let mut dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout());
    let resources: HashMap<_, _> = HashMap::new();
    let mut renderer = BrushPreviewRenderer::new();
    let graph = default_graph();

    let result = renderer.render_stroke(
        &device,
        &queue,
        &mut dab_pool,
        &pipelines,
        &resources,
        &graph,
        &[],
        [1.0, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        320,
        120,
    );
    assert!(result.is_none());
}
