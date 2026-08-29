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
use darkly::gpu::preview::{pixel_centre, PreviewBackdrop};
use darkly::gpu::test_utils::test_device;

/// Brushes whose graphs sample the canvas, and the glyph each declares.
const STAGED: [(&str, &str); 4] = [
    ("Liquify", "tabler:ripple"),
    ("Smudge", "mdi:gesture-swipe"),
    ("Blur", "mdi:blur"),
    ("Clone", "fa6-solid:clone"),
];

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn fresh_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);
    engine.set_preview_theme(WHITE, BLACK);
    engine
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

/// Standard deviation of luminance. `0.0` means every pixel is the same colour.
fn luminance_sd(pixels: &[u8]) -> f32 {
    let lums: Vec<f32> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| luminance(px))
        .collect();
    let mean = lums.iter().sum::<f32>() / lums.len() as f32;
    (lums.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lums.len() as f32).sqrt()
}

/// Comfortably above readback and resize noise, and far below the ~34 levels of
/// standard deviation the staged bands alone carry.
const VISIBLE: f32 = 12.0;

/// What the stroke did, isolated from what it was staged over.
///
/// The **raw** render canvas, before the framer crops it, compared pixel by
/// pixel against a CPU evaluation of the backdrop the render was staged over —
/// the same `sample()` the framer itself compares against, at the same
/// tolerance. Every texel the stroke did not touch round-trips through
/// `write_texture` → `save_pre_stroke` → `color_output::commit` bit-identically,
/// so a pixel outside the tolerance here is the stroke and nothing else.
///
/// That is what makes this backdrop-agnostic: a preview that shows nothing but
/// its own backdrop scores exactly zero whatever the backdrop is, and swapping
/// the field for the next one moves the numbers without invalidating the idea.
struct Stroke {
    /// Pixels the stroke changed, as a fraction of the render canvas.
    fraction: f32,
    /// Their bounding box, in render pixels.
    bbox: (u32, u32),
}

fn measure_stroke(engine: &mut DarklyEngine, backdrop: PreviewBackdrop) -> Stroke {
    /// The framer's own tolerance — accommodates premultiplied-alpha rounding.
    const TOLERANCE: i32 = 12;
    let (pixels, w, h) = engine.test_render_stroke_preview_canvas();
    let mut changed = 0usize;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (u, v) = pixel_centre(x, y, w, h);
            let want = backdrop.sample(u, v, WHITE, BLACK);
            let differs = (0..3).any(|c| {
                let want = (want[c].clamp(0.0, 1.0) * 255.0).round() as i32;
                (pixels[i + c] as i32 - want).abs() > TOLERANCE
            });
            if differs {
                changed += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    Stroke {
        fraction: changed as f32 / (w * h) as f32,
        bbox: if changed == 0 {
            (0, 0)
        } else {
            (max_x - min_x + 1, max_y - min_y + 1)
        },
    }
}

/// Floor on how much of the render canvas the stroke must change.
///
/// Measured over the shipped backdrop: Blur — the weakest of the four, and the
/// one this floor exists for — changes 0.121 %, clearing it by 2.4×, and
/// Liquify, Smudge and Clone clear it by 8.8× / 15.9× / 29.6×. Blur *without*
/// its preview pin changes 0.022 %, less than half the floor.
///
/// A stripe field only responds where an operator's action crosses a band edge,
/// so these numbers are much closer together than they would be over a field
/// carrying every spatial frequency. That is the accepted cost of a backdrop
/// whose *rendered* strokes read better; see [`PreviewBackdrop::Stripes`].
const MIN_CHANGED_FRACTION: f32 = 0.0005;

/// Floor on the stroke's bounding box, in render pixels.
///
/// The framer crops to this box, so it is the only machine check left on what
/// the thumbnail is a picture *of*: a stroke that only registers in patches
/// crops to a fragment blown up to fill the tile, which is how the original bug
/// looked once it stopped being invisible — unpinned Blur's 194 × 35 becomes a
/// tile showing two stripes and no stroke. Unlike the fraction above it cannot
/// carry a large margin: the S-curve's own extent is the ceiling, and the
/// weakest brush measures 323 × 49 against Liquify's 397 × 97.
const MIN_BBOX: (u32, u32) = (256, 40);

/// **The regression.** Every content-dependent brush's stroke preview shows a
/// stroke: measured against its own backdrop, on the canvas the framer reads,
/// there is one and it spans the path.
#[test]
fn content_dependent_brushes_render_a_visible_stroke() {
    let mut engine = fresh_engine();
    for (name, _) in STAGED {
        engine
            .brush_load(name)
            .unwrap_or_else(|e| panic!("'{name}' is a built-in brush: {e}"));
        let backdrop =
            darkly::brush::graph_capabilities(&engine.active_brush_graph()).preview_backdrop;
        let stroke = measure_stroke(&mut engine, backdrop);
        assert!(
            stroke.fraction > MIN_CHANGED_FRACTION,
            "'{name}' changed {:.4}% of its preview canvas — its stroke is its \
             own backdrop showing through",
            stroke.fraction * 100.0,
        );
        assert!(
            stroke.bbox.0 >= MIN_BBOX.0 && stroke.bbox.1 >= MIN_BBOX.1,
            "'{name}' changed only a {}x{} patch, so the framer crops a \
             fragment rather than the stroke",
            stroke.bbox.0,
            stroke.bbox.1,
        );
    }
}

/// **Part of the same regression, and the feature test for the preview pin.**
/// Blur's shipped strength puts a sub-pixel kernel against a ~36 px preview dab,
/// so the stroke registers only in patches and the framer crops one of them.
/// The port declares a `preview_value` so the preview renders at a strength that
/// marks the whole S-curve. Without it Blur fails both thresholds above.
#[test]
fn the_preview_pin_is_what_makes_blur_read() {
    let mut engine = fresh_engine();
    engine.brush_load("Blur").expect("Blur is a built-in brush");

    let pinned = darkly::brush::registry()
        .get("blur")
        .expect("the blur node is registered")
        .node
        .ports
        .iter()
        .find(|p| p.name == "strength")
        .expect("blur declares a strength port")
        .preview_value;
    let pinned = pinned.expect("blur.strength is pinned for previews");
    assert!(
        pinned > 0.05,
        "a pin at or below the shipped default would render the same \
         sub-pixel kernel the preview cannot show"
    );

    let stroke = measure_stroke(&mut engine, PreviewBackdrop::Stripes);
    assert!(
        stroke.fraction > 2.0 * MIN_CHANGED_FRACTION,
        "Blur's pinned preview changed {:.4}% of the canvas — the pin is \
         supposed to leave the floor twice as much headroom as it needs",
        stroke.fraction * 100.0,
    );
    assert!(
        stroke.bbox.0 >= 320 && stroke.bbox.1 >= 45,
        "Blur's pinned preview marks a {}x{} box — the pin exists to grow it \
         past the point where the framer crops a fragment, which the strength \
         sweep puts at ~0.15",
        stroke.bbox.0,
        stroke.bbox.1,
    );
}

/// Every brush that deposits pigment keeps the flat clear, so nothing about its
/// preview changes. Nine of the thirteen shipped brushes.
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
    assert_eq!(flat, 9, "nine shipped brushes deposit pigment");
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
            darkly::brush::library::BrushInfo::from(brush).icon,
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
    engine.set_preview_theme(WHITE, BLACK);
    let (light_on_dark, _, _) = stroke_thumbnail(&mut engine, "Liquify");

    engine.set_preview_theme(BLACK, WHITE);
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
    for i in 0..64 {
        let u = i as f32 / 64.0;
        assert_ne!(
            PreviewBackdrop::Stripes.sample(u, 0.5, WHITE, BLACK),
            PreviewBackdrop::Stripes.sample(u + du, 0.5, WHITE, BLACK),
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
        "Ink Pen",
        "Clone",
    ] {
        let (first, _, _) = stroke_thumbnail(&mut engine, name);
        // Drop the bake and take it again from scratch.
        engine.set_preview_theme([0.5, 0.5, 0.5, 1.0], [0.25, 0.25, 0.25, 1.0]);
        engine.set_preview_theme(WHITE, BLACK);
        let (second, _, _) = stroke_thumbnail(&mut engine, name);
        assert_eq!(first, second, "'{name}' renders differently every bake");
    }
}
