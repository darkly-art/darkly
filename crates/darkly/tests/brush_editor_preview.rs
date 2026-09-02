//! Native-only integration tests for the full-stroke brush editor preview.
//!
//! Asserts the stroke engine runs end-to-end against the preview's own
//! scratch target and produces non-empty RGBA pixels where the S-curve was
//! drawn. Uses the blocking `test_utils::readback_texture` helper (native
//! only); the wasm path does async readback via the ReadbackScheduler.

use darkly::brush::{
    default_graph,
    pipeline::BrushPipelines,
    preview_renderer::{synthesize_stroke_path, BrushStrokePreviewRenderer},
};
use darkly::gpu::preview::PreviewBackdrop;
use darkly::gpu::test_utils::{readback_texture, test_device};

#[test]
fn renders_s_curve_over_black_background() {
    let (device, queue) = test_device();
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut renderer = BrushStrokePreviewRenderer::new();
    let graph = default_graph();

    let width: u32 = 320;
    let height: u32 = 120;
    let path = synthesize_stroke_path(width as f32, height as f32, 30, 0.0);

    let fg = [1.0, 1.0, 1.0, 1.0]; // white stroke
    let bg = [0.0, 0.0, 0.0, 1.0]; // black background

    let texture = renderer
        .render_stroke(
            &device,
            &queue,
            &pipelines,
            &graph,
            &path,
            fg,
            bg,
            PreviewBackdrop::Flat,
            width,
            height,
            None,
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
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut renderer = BrushStrokePreviewRenderer::new();
    let graph = default_graph();
    let path = synthesize_stroke_path(320.0, 120.0, 20, 0.0);

    assert!(renderer.current_size().is_none());

    let _ = renderer.render_stroke(
        &device,
        &queue,
        &pipelines,
        &graph,
        &path,
        [1.0, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        PreviewBackdrop::Flat,
        320,
        120,
        None,
    );
    assert_eq!(renderer.current_size(), Some((320, 120)));
    let first_ptr = renderer.current_texture().map(|t| t as *const _);

    let _ = renderer.render_stroke(
        &device,
        &queue,
        &pipelines,
        &graph,
        &path,
        [1.0, 0.0, 0.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
        PreviewBackdrop::Flat,
        320,
        120,
        None,
    );
    let second_ptr = renderer.current_texture().map(|t| t as *const _);

    // Same size → same underlying texture.
    assert_eq!(first_ptr, second_ptr);
}

/// Decode a `brush_stroke_preview()` PNG to raw RGBA bytes plus its
/// canonical `BRUSH_THUMBNAIL_SIZE` dimensions: same shape the frontend
/// receives via the `Blob` URL path.
fn decode_preview_png(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let img = image::load_from_memory(png_bytes).expect("valid PNG bytes");
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    (w, h, rgba.into_raw())
}

#[test]
fn engine_brush_stroke_preview_caches_after_readback() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // First call: cache empty, kicks off a readback, returns an empty Vec.
    // The frontend uses that as a "no fresh bytes" signal so it
    // preserves whatever was last shown rather than flashing transparent.
    let first = engine.brush_stroke_preview();
    assert!(
        first.is_empty(),
        "cache miss should return empty Vec, got {} bytes",
        first.len()
    );

    // Flush the in-flight readback (native-only helper; wasm relies on the
    // event loop polling the ReadbackScheduler via the render loop).
    engine.test_flush_readbacks();

    // Second call: cache now populated with PNG bytes, same shape as
    // `brush_active_dab_preview` / `brush_thumbnail`.
    let second = engine.brush_stroke_preview();
    assert!(
        !second.is_empty(),
        "post-readback call should return cached PNG bytes"
    );
    assert_eq!(
        &second[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "cache should hold PNG-encoded bytes"
    );

    // Decode and verify the stroke deposited ink.
    let (w, h, pixels) = decode_preview_png(&second);
    assert!(w > 0 && h > 0, "decoded preview has positive dimensions");
    let non_zero_pixels = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] > 0 || px[1] > 0 || px[2] > 0)
        .count();
    assert!(
        non_zero_pixels > 100,
        "expected non-trivial stroke coverage in cached preview, got {non_zero_pixels} non-zero pixels"
    );
}

#[test]
fn engine_brush_stroke_preview_skips_unchanged_graph() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Prime the cache.
    let _ = engine.brush_stroke_preview();
    engine.test_flush_readbacks();
    let first = engine.brush_stroke_preview();

    // Without touching the graph, a second call returns the same cache
    // and does not queue another readback.
    let second = engine.brush_stroke_preview();
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
    let _ = engine.brush_stroke_preview();
    engine.test_flush_readbacks();
    let dark_png = engine.brush_stroke_preview();

    // Switch to the light theme: black on light. Cache should invalidate
    // and the next readback should produce distinctly different pixels.
    engine.set_preview_theme([0.0, 0.0, 0.0, 1.0], [0.9, 0.9, 0.9, 1.0]);
    let after_change = engine.brush_stroke_preview();
    // Pre-readback call returns an empty Vec (cache was invalidated).
    assert!(after_change.is_empty());

    engine.test_flush_readbacks();
    let light_png = engine.brush_stroke_preview();

    assert_ne!(
        dark_png, light_png,
        "theme change must produce new preview bytes"
    );
    // Sanity-check: the light-theme preview has bright bg pixels.
    let (_, _, light_pixels) = decode_preview_png(&light_png);
    let mut bright_bg = 0;
    for chunk in light_pixels.as_chunks::<4>().0 {
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
    use darkly::brush::library;
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Save a brush: kicks off an async thumbnail readback against the
    // process-wide library.
    engine.brush_save("test_brush", "TestBrush").unwrap();

    // Before the readback lands, the library entry has no thumbnail.
    assert!(
        library::with(|lib| lib.thumbnail_png("test_brush").is_none()),
        "thumbnail should be absent before readback completes"
    );

    // Flush the pending readback; the completion handler writes the PNG
    // back onto the library entry.
    engine.test_flush_readbacks();

    let png = library::with(|lib| {
        lib.thumbnail_png("test_brush")
            .expect("thumbnail present after readback")
            .to_vec()
    });
    // Valid PNG: starts with the PNG magic signature.
    assert_eq!(
        &png[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "library entry now carries a PNG-encoded thumbnail"
    );
}

/// Regression: Hard Round (no `pressure → size_input` wire) paints every
/// dab at full size, including the endpoints. The endpoint dabs must not
/// be clipped against the cache border: the leftmost and rightmost
/// columns of the framed preview must contain only background pixels.
///
/// This is the artist-visible bug: with the previous size-aware inset
/// hack, the path was shrunk so endpoints landed inside the canvas,
/// but the inset clamped to half the canvas at any non-trivial size and
/// the path degenerated. Without an inset, endpoints sit on the canvas
/// edge and the framer can't recover the clipped half of the dab.
#[test]
fn airbrush_endpoint_dabs_not_clipped_against_cache_border() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Pin the theme so the bg pixel value is deterministic for the test:
    // black bg, white stroke.
    engine.set_preview_theme([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);

    // Airbrush is a built-in: shape tip with a fixed `size_input` constant
    // (no pressure→size_input wire), so the dab radius doesn't scale with
    // the synthetic stroke's pressure ramp. Same invariant the old
    // "Hard Round" test exercised before that brush was removed.
    engine.brush_load("Airbrush").expect("Airbrush built-in");

    // Prime + flush + read.
    let _ = engine.brush_stroke_preview();
    engine.test_flush_readbacks();
    let png = engine.brush_stroke_preview();
    let (width, height, pixels) = decode_preview_png(&png);
    assert_eq!(pixels.len(), (width * height * 4) as usize);

    // bg is black; mark anything noticeably brighter as stroke.
    const TOLERANCE: u8 = 16;
    let is_stroke = |i: usize| -> bool {
        pixels[i] > TOLERANCE || pixels[i + 1] > TOLERANCE || pixels[i + 2] > TOLERANCE
    };

    // The leftmost and rightmost columns must be entirely background:
    // any stroke pixel there means an endpoint dab was clipped.
    let edge_band = 1u32;
    for x_band in [0..edge_band, (width - edge_band)..width] {
        for x in x_band {
            for y in 0..height {
                let i = ((y * width + x) * 4) as usize;
                assert!(
                    !is_stroke(i),
                    "Airbrush preview cuts off at the edge: column {x} y={y} \
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
/// change in response to an artist scrub. Bumping `brush_graph_version`
/// on these scrubs would trigger a wasted full-stroke re-render every
/// 100 ms while the artist drags the slider (~1 GB/s of GPU work for no
/// visible effect).
///
/// The fix declares stabilize via `preview_irrelevant_scrub()` and routes
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
    // one in the same engine, which avoids creating a second wgpu device.
    engine.brush_load("Ink Pen").expect("Ink Pen built-in");

    // Prime the editor preview cache and let the readback land so the
    // version counter is at its post-init steady state.
    let _ = engine.brush_stroke_preview();
    engine.test_flush_readbacks();
    let v_before_stabilize = engine.brush_graph_version();

    // Find the exposed `stabilize` port and scrub it through
    // `brush_set_exposed_port`: the same entry point the brush bar uses.
    let stabilize = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.port_name == "stabilize")
        .expect("Ink Pen exposes a `stabilize` port");
    engine
        .brush_set_exposed_port(&stabilize.node_id, "stabilize", 90.0)
        .expect("scrub set");

    assert_eq!(
        engine.brush_graph_version(),
        v_before_stabilize,
        "stabilize is preview-irrelevant (PassThrough is hardcoded for \
         the synthetic stroke); scrubbing it must not bump \
         brush_graph_version: bumping invalidates the editor preview \
         cache and triggers a wasted full-stroke re-render."
    );

    // Negative control: scrubbing a port the preview *does* read must
    // still bump the version. After the compiled-WGSL migration
    // `softness` lives on the upstream `circle` node (the
    // `paint` terminal has no softness port). It has no
    // `preview_irrelevant_scrub` flag, is read by the preview shader,
    // and is unwired: the perfect canary for "rule too broad". Find
    // its node via the exposed-port listing.
    let softness = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.port_name == "softness")
        .expect("Ink Pen exposes a `softness` port (on shape after migration)");
    let v_before_softness = engine.brush_graph_version();
    engine
        .brush_set_exposed_port(&softness.node_id, "softness", 0.5)
        .expect("scrub set");
    assert_ne!(
        engine.brush_graph_version(),
        v_before_softness,
        "softness is not flagged preview_irrelevant_scrub → it affects \
         the preview output → its scrub must bump brush_graph_version. \
         If this assertion fails, the preview-irrelevant rule is \
         over-broad and real preview updates would also stall."
    );
}

/// Regression: scrubbing `brush_settings.size` (or any port flagged with
/// `preview_value`) must not invalidate the editor-preview cache.
///
/// The previous "continued charcoal debugging" attempt deleted the
/// caller-side `graph.apply_preview_overrides()` on the stroke-preview
/// path and rewrote the `ChangeKind` classifier to key off
/// `preview_irrelevant_scrub` only, with the rationale "the preview
/// must match what the artist would actually paint". That made the
/// brush picker tile and the editor stroke preview inconsistent:
/// the tile stayed size-invariant via `reset_exposed_scrubs`, but the
/// stroke preview now mutated visibly on every size scrub.
///
/// Both previews share the same intent: brush identity, not momentary
/// scrub state. This test pins the restored behavior: a scrub of a pinned
/// port on the active brush must not bump `brush_graph_version` (which
/// is what gates the editor preview cache), because the renderer
/// neutralizes `preview_value` ports before rendering, so the output
/// is identical anyway.
///
/// `blur.strength` is covered alongside `brush_settings.size` because the
/// two pins cost different things and only one of them is free. A size
/// scrub was never legible in a preview rendered at a canonical size, so
/// nothing was lost; a strength scrub *was* visible before the port was
/// pinned, and freezing it is a deliberate trade: a stroke preview says
/// what kind of brush this is, not what one parameter is currently set to.
/// Asserting it here is what records that as intended rather than
/// accidental.
#[test]
fn pinned_ports_are_pinned_in_previews() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    for (brush, port, scrub_to) in [("Ink Pen", "size", 90.0), ("Blur", "strength", 90.0)] {
        engine
            .brush_load(brush)
            .unwrap_or_else(|e| panic!("'{brush}' is a built-in brush: {e}"));
        let _ = engine.brush_stroke_preview();
        engine.test_flush_readbacks();
        let v_before = engine.brush_graph_version();

        let exposed = engine
            .brush_exposed_ports()
            .into_iter()
            .find(|p| p.port_name == port)
            .unwrap_or_else(|| panic!("'{brush}' exposes a `{port}` port"));
        engine
            .brush_set_exposed_port(&exposed.node_id, port, scrub_to)
            .expect("scrub set");

        assert_eq!(
            engine.brush_graph_version(),
            v_before,
            "scrubbing '{brush}' `{port}` must not bump brush_graph_version. \
             The port is flagged `preview_value`, and the stroke-preview render \
             path applies `apply_preview_overrides` to neutralize it before \
             rendering, so the editor preview's output cannot change in \
             response to the scrub, and invalidating its cache would just \
             trigger a wasted full-stroke re-render and a visible blink as the \
             new value briefly shows."
        );
    }
}

/// Assert no pixel on the outermost border row/column carries ink (i.e. the
/// rendered content stayed clear of the render-canvas edge and nothing was
/// clipped away before the framer's changed-pixel crop ran). `bg` is assumed
/// near-black (the tests pin a black theme bg), so any bright border pixel is
/// clipped stroke.
fn assert_no_ink_on_render_border(pixels: &[u8], w: u32, h: u32, label: &str) {
    assert_eq!(pixels.len(), (w * h * 4) as usize);
    const TOLERANCE: u8 = 16;
    let is_ink = |x: u32, y: u32| -> bool {
        let i = ((y * w + x) * 4) as usize;
        pixels[i] > TOLERANCE || pixels[i + 1] > TOLERANCE || pixels[i + 2] > TOLERANCE
    };
    for x in 0..w {
        for y in [0, h - 1] {
            assert!(
                !is_ink(x, y),
                "{label}: ink clipped against render border at ({x}, {y}) rgba={:?}",
                {
                    let i = ((y * w + x) * 4) as usize;
                    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
                }
            );
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            assert!(
                !is_ink(x, y),
                "{label}: ink clipped against render border at ({x}, {y}) rgba={:?}",
                {
                    let i = ((y * w + x) * 4) as usize;
                    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
                }
            );
        }
    }
}

/// Squash the active brush's tip to the extreme calligraphy nib (aspect
/// 10% → ~10× anisotropy, the worst case the preview canvas is sized for)
/// by scrubbing its exposed `aspect` port. Returns nothing; mutates the
/// active graph in place.
fn squash_active_brush_to_extreme_nib(engine: &mut darkly::engine::DarklyEngine) {
    let aspect = engine
        .brush_exposed_ports()
        .into_iter()
        .find(|p| p.port_name == "aspect")
        .expect("Calligraphy exposes an `aspect` port");
    // Percent unit: display 10 → port 0.1 (the port's minimum, ~10× stretch).
    engine
        .brush_set_exposed_port(&aspect.node_id, "aspect", 10.0)
        .expect("scrub aspect to 10%");
}

/// Regression: a broad-nib calligraphy tip must not be clipped against the
/// stroke-preview render canvas. The tip's anisotropy stretches the dab
/// footprint up to ~10×, so a fixed small canvas + absolute inset clipped the
/// endpoints (and even mid-stroke edges) before the changed-pixel crop ever
/// saw them: the artist-reported "calligraphy stroke is cut off" bug.
///
/// The invariant is size-agnostic: whatever canvas the preview pipeline picks,
/// the neutralized preview stroke must stay clear of its border. We assert
/// directly on the raw render canvas (pre-crop) via the test accessor.
#[test]
fn calligraphy_stroke_preview_not_clipped_against_render_border() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    // Pin a black bg / white stroke so border ink is unambiguous.
    engine.set_preview_theme([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
    engine
        .brush_load("Calligraphy")
        .expect("Calligraphy built-in");
    squash_active_brush_to_extreme_nib(&mut engine);

    let (pixels, w, h) = engine.test_render_stroke_preview_canvas();
    assert_no_ink_on_render_border(&pixels, w, h, "calligraphy stroke preview");
}

/// Regression: the same broad-nib calligraphy tip must not be clipped against
/// the single-dab preview render canvas either. `aspect` is
/// `persist_in_thumbnail` and not a default-exposed scrub, so it survives the
/// dab path's `reset_exposed_scrubs`: the nib the picker tile shows is the
/// full ellipse, not a truncated one.
#[test]
fn calligraphy_dab_preview_not_clipped_against_render_border() {
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    engine.set_preview_theme([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
    engine
        .brush_load("Calligraphy")
        .expect("Calligraphy built-in");
    squash_active_brush_to_extreme_nib(&mut engine);

    let (pixels, w, h) = engine.test_render_dab_preview_canvas();
    assert_no_ink_on_render_border(&pixels, w, h, "calligraphy dab preview");
}

#[test]
fn empty_path_returns_none() {
    let (device, queue) = test_device();
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut renderer = BrushStrokePreviewRenderer::new();
    let graph = default_graph();

    let result = renderer.render_stroke(
        &device,
        &queue,
        &pipelines,
        &graph,
        &[],
        [1.0, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        PreviewBackdrop::Flat,
        320,
        120,
        None,
    );
    assert!(result.is_none());
}
