//! A brush whose output depends on canvas content it did not write must still
//! show a stroke in its preview.
//!
//! Four shipped brushes — Liquify, Smudge, Blur, Clone — transport the
//! destination rather than writing to it. Over the flat preview background they
//! transported a constant and their baked stroke thumbnails were, pixel for
//! pixel, that same constant. The fix is a field declared by the node and
//! painted under the stroke; these tests are what holds it.

use darkly::brush::builtin_brushes;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::preview::PreviewBackdrop;
use darkly::gpu::test_utils::test_device;

/// Brushes whose graphs sample the canvas, and the glyph each declares.
const STAGED: [(&str, &str); 4] = [
    ("Liquify", "tabler:ripple"),
    ("Smudge", "mdi:gesture-swipe"),
    ("Blur", "mdi:blur"),
    ("Clone", "fa6-solid:clone"),
];

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 1024, 768)
}

/// One brush's framed stroke thumbnail, decoded to RGBA8.
fn stroke_thumbnail(engine: &mut DarklyEngine, name: &str) -> (Vec<u8>, u32, u32) {
    let _ = engine.brush_thumbnail(name);
    engine.test_flush_readbacks();
    let png = engine.brush_thumbnail(name);
    assert!(!png.is_empty(), "no thumbnail baked for '{name}'");
    let img = image::load_from_memory(&png)
        .unwrap_or_else(|e| panic!("'{name}' thumbnail is not a valid PNG: {e}"))
        .to_rgba8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

fn luminance(px: &[u8]) -> f32 {
    0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32
}

/// Standard deviation of luminance. `0.0` means every pixel is the same colour
/// — the symptom this whole change exists to remove.
fn luminance_sd(pixels: &[u8]) -> f32 {
    let lums: Vec<f32> = pixels.chunks_exact(4).map(luminance).collect();
    let mean = lums.iter().sum::<f32>() / lums.len() as f32;
    (lums.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lums.len() as f32).sqrt()
}

/// The largest luminance range found within any single column.
///
/// The load-bearing statistic. [`PreviewBackdrop::Stripes`] is vertical bands —
/// constant in `v` — and cropping and resizing preserve that, so a staged
/// preview that showed nothing but its own backdrop would read `0.0` here no
/// matter how much horizontal contrast the stripes carry. Anything above the
/// noise floor is the stroke itself.
fn max_column_range(pixels: &[u8], w: u32, h: u32) -> f32 {
    (0..w)
        .map(|x| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for y in 0..h {
                let i = ((y * w + x) * 4) as usize;
                let l = luminance(&pixels[i..i + 4]);
                lo = lo.min(l);
                hi = hi.max(l);
            }
            hi - lo
        })
        .fold(0.0, f32::max)
}

/// Comfortably above readback and resize noise, comfortably below the ~43 the
/// weakest already-working brush (Smooth Watercolor) measures.
const VISIBLE: f32 = 12.0;

/// **The regression.** Every content-dependent brush's stroke preview shows a
/// stroke: it is not one flat colour, and — the part its own backdrop cannot
/// fake — it varies down a column.
#[test]
fn content_dependent_brushes_render_a_visible_stroke() {
    let mut engine = fresh_engine();
    for (name, _) in STAGED {
        let (pixels, w, h) = stroke_thumbnail(&mut engine, name);
        assert!(
            luminance_sd(&pixels) > VISIBLE,
            "'{name}' stroke preview is flat (luminance SD {:.2})",
            luminance_sd(&pixels)
        );
        assert!(
            max_column_range(&pixels, w, h) > VISIBLE,
            "'{name}' stroke preview varies only horizontally ({:.2}) — that is \
             its staged backdrop showing through, not a stroke",
            max_column_range(&pixels, w, h)
        );
    }
}

/// Every brush that deposits pigment keeps the flat clear, so nothing about its
/// preview changes. Ten of the fourteen shipped brushes.
#[test]
fn depositing_brushes_stage_nothing() {
    let staged: Vec<&str> = STAGED.iter().map(|(n, _)| *n).collect();
    let mut flat = 0;
    for brush in builtin_brushes::all() {
        let caps = darkly::brush::graph_capabilities(&brush.metadata.graph);
        let name = &brush.metadata.name;
        if staged.contains(&name.as_str()) {
            assert_eq!(
                caps.preview_backdrop,
                PreviewBackdrop::Stripes,
                "'{name}' samples the canvas and must be staged"
            );
        } else {
            assert_eq!(
                caps.preview_backdrop,
                PreviewBackdrop::Flat,
                "'{name}' deposits pigment and must keep the flat clear"
            );
            flat += 1;
        }
    }
    assert_eq!(flat, 10, "ten shipped brushes deposit pigment");
}

/// A `Flat` backdrop is the theme background at every position — which is what
/// makes the fast path bit-identical to the clear it replaced, and what lets the
/// framer evaluate `sample()` unconditionally instead of branching.
#[test]
fn flat_is_the_background_everywhere() {
    let fg = [1.0, 1.0, 1.0, 1.0];
    let bg = [0.1, 0.2, 0.3, 1.0];
    for i in 0..32 {
        let (u, v) = (i as f32 / 32.0, (31 - i) as f32 / 32.0);
        assert_eq!(PreviewBackdrop::Flat.sample(u, v, fg, bg), bg);
    }
}

/// The dab slot is the glyph. A single stationary sample has no motion for a
/// displacement to reveal, so these four brushes show their declared icon there
/// rather than a bake — and the icon is what `BrushInfo` projects to the picker.
#[test]
fn the_dab_slot_belongs_to_the_icon() {
    let brushes = builtin_brushes::all();
    for (name, icon) in STAGED {
        let brush = brushes
            .iter()
            .find(|b| b.metadata.name == name)
            .unwrap_or_else(|| panic!("built-in brush '{name}' must exist"));
        assert_eq!(
            darkly::brush::graph_capabilities(&brush.metadata.graph).preview_fallback_icon,
            Some(icon),
        );
        assert_eq!(
            darkly::brush::library::BrushInfo::from(&brush.metadata).icon,
            Some(icon),
            "'{name}' projects its glyph to the picker"
        );
    }
}

/// The backdrop is a function of the theme poles, not of hard-coded greys.
/// Inverting the theme must invert the staging with it — `set_preview_theme`
/// already drops every cached thumbnail, so nothing else has to invalidate.
#[test]
fn the_backdrop_follows_the_theme() {
    let mut engine = fresh_engine();
    engine.set_preview_theme([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
    let (light_on_dark, _, _) = stroke_thumbnail(&mut engine, "Liquify");

    engine.set_preview_theme([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
    let (dark_on_light, _, _) = stroke_thumbnail(&mut engine, "Liquify");

    assert!(luminance_sd(&light_on_dark) > VISIBLE);
    assert!(luminance_sd(&dark_on_light) > VISIBLE);
    assert_ne!(
        light_on_dark, dark_on_light,
        "the staged backdrop ignores the theme"
    );
}

/// A node that transports pixels from elsewhere needs an offset that *escapes*
/// the field it is transporting: a whole number of stripe periods reproduces the
/// backdrop exactly and the clone stays invisible. Half a period is the furthest
/// from that, and there is no vertical component because the field has no
/// vertical structure to escape.
///
/// A pure unit test, deliberately — this is the property that a GPU render
/// cannot distinguish from a working clone until someone looks at the PNG.
#[test]
fn the_clone_offset_escapes_the_stripes() {
    let [du, dv] = PreviewBackdrop::Stripes.source_offset();
    assert_eq!(dv, 0.0, "a vertical offset over vertical bands is a no-op");

    // Stated against the field rather than against its period, so it holds
    // whatever `BANDS` is set to: shifting by the offset must land on the other
    // tone *everywhere*, which is what "exactly out of phase" means and what an
    // offset of a whole period fails at every position.
    let fg = [1.0, 1.0, 1.0, 1.0];
    let bg = [0.0, 0.0, 0.0, 1.0];
    for i in 0..64 {
        let u = i as f32 / 64.0;
        assert_ne!(
            PreviewBackdrop::Stripes.sample(u, 0.5, fg, bg),
            PreviewBackdrop::Stripes.sample(u + du, 0.5, fg, bg),
            "at u = {u} the offset {du} lands back on the same band, so a clone \
             of the backdrop is the backdrop"
        );
    }

    assert_eq!(
        PreviewBackdrop::Flat.source_offset(),
        [0.0, 0.0],
        "a flat field has nothing to escape"
    );
}

/// A node that needs staging needs both halves of it. Nearly tautological now
/// that they are one struct — which is the point: it records why they are one,
/// and it is the check that would have been load-bearing had they stayed two
/// fields that could drift apart.
#[test]
fn every_declaring_node_declares_both_halves() {
    for reg in darkly::brush::registry().types() {
        let Some(staging) = reg.node.preview_staging else {
            continue;
        };
        assert!(
            !staging.icon.is_empty(),
            "'{}' declares staging with no glyph for the dab slot",
            reg.node.type_id
        );
        assert_ne!(
            staging.backdrop,
            PreviewBackdrop::Flat,
            "'{}' declares staging that stages nothing",
            reg.node.type_id
        );
    }
}

/// A preview is a picture of a brush, not of one stroke of it. Five shipped
/// brushes contain `random`/`noise` nodes, and until the stroke seed became the
/// caller's to choose they rendered differently every time — which would have
/// made a cached thumbnail differ from its own re-bake and a documentation asset
/// churn on every rebuild.
#[test]
fn previews_are_reproducible() {
    let mut engine = fresh_engine();
    for name in [
        "Rough Ink",
        "Rough Watercolor",
        "Smooth Watercolor",
        "Round",
    ] {
        let (first, _, _) = stroke_thumbnail(&mut engine, name);
        // Drop the bake and take it again from scratch.
        engine.set_preview_theme([0.5, 0.5, 0.5, 1.0], [0.25, 0.25, 0.25, 1.0]);
        engine.set_preview_theme([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
        let (second, _, _) = stroke_thumbnail(&mut engine, name);
        assert_eq!(first, second, "'{name}' renders differently every bake");
    }
}
