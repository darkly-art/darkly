//! Regression test for cursor-preview coverage normalization.
//!
//! Before this lands, the dab-tile bake path (`frame_dab_thumbnail`) ran
//! a Photoshop-Levels auto-brighten on the readback pixels and the
//! cursor-follow overlay got no equivalent boost — so attenuated brushes
//! (charcoal: paper × shape ≈ 0.2 mean coverage) had a visible bake
//! tile but a barely-visible cursor halo. The fix flips the wiring: the
//! bake stays at the brush's natural intensity, and the cursor overlay
//! shader multiplies sampled coverage by a per-brush scale computed via
//! async readback of the freshly-rendered preview mask.
//!
//! What this test pins down:
//!   - Charcoal recompile → readback → cached scale lifts the cursor
//!     overlay above an explicit visibility threshold.
//!   - Ink Pen (already at near-full coverage) stays at ≈1.0 — no
//!     over-boost.
//!   - The scale tracks brush identity: switching from charcoal to
//!     ink pen recomputes back down rather than carrying the charcoal
//!     boost forward.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;

fn engine_for_brush(name: &str) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);
    engine.brush_load(name).expect("brush in library");
    // The constructor already kicks off one regenerate; loading a brush
    // bumps the topology and triggers another. Flushing twice settles
    // both the initial-graph readback and the loaded-brush readback.
    engine.test_flush_readbacks();
    engine.test_flush_readbacks();
    engine
}

#[test]
fn charcoal_cursor_preview_scale_lifts_above_visibility_floor() {
    let engine = engine_for_brush("Charcoal");
    let scale = engine.cursor_preview_coverage_scale();
    // Charcoal's natural mask average (~0.2) needs a boost of roughly
    // 130/255 ÷ 0.2 ≈ 2.5× to hit the target. A scale below 1.5 means
    // the normalize didn't kick in; that's exactly the prior bug where
    // the halo was barely visible.
    assert!(
        scale > 1.5,
        "expected charcoal cursor preview to be boosted above 1.5×, \
         got scale={scale}. The cursor overlay would be nearly \
         invisible at this scale."
    );
    // Cap is 8.0; if we hit it the brush rendered as effectively empty
    // and the normalize ran wild — also a failure mode.
    assert!(
        scale < 8.0,
        "scale capped at MAX_BOOST=8.0 → preview mask was nearly empty; \
         got scale={scale}"
    );
}

#[test]
fn ink_pen_cursor_preview_scale_stays_near_unity() {
    let engine = engine_for_brush("Ink Pen");
    let scale = engine.cursor_preview_coverage_scale();
    // Ink Pen is a natural full-coverage brush — its mask averages well
    // above the target so the formula short-circuits to 1.0 (no boost).
    // A scale above ~1.2 here would mean either the brush regressed
    // toward partial coverage or the formula over-boosts.
    assert!(
        (scale - 1.0).abs() < 0.05,
        "expected ink pen cursor preview to stay near 1.0× (natural \
         full coverage), got scale={scale}"
    );
}

/// Defense-in-depth: the engine tests above verify the scale uniform is
/// computed correctly, but the visible bug (dim halo) only surfaces if
/// the fragment shader actually multiplies sampled coverage by that
/// uniform. A drive-by "fix" that drops the multiplication would
/// silently restore the bug; this test catches it by inspecting the
/// shader source.
#[test]
fn overlay_shader_multiplies_coverage_by_preview_scale() {
    let src = include_str!("../../../shaders/overlay.wgsl");
    assert!(
        src.contains("preview_coverage_scale"),
        "overlay.wgsl must reference preview_coverage_scale — the \
         coverage normalization that lifts attenuated brushes to \
         visible brightness depends on it."
    );
    // Sanity-check the multiplication site itself. The line must occur
    // in the KIND_MASKED_STAMP branch; we look for the exact production
    // pattern so a reorder/rename in that branch trips the test.
    assert!(
        src.contains("raw * u.preview_coverage_scale"),
        "the KIND_MASKED_STAMP branch must multiply sampled mask \
         coverage by u.preview_coverage_scale — otherwise the cursor \
         halo dims back to the pre-fix bug."
    );
}

#[test]
fn switching_brushes_recomputes_scale() {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = DarklyEngine::new(gpu, 1024, 768);

    engine.brush_load("Charcoal").unwrap();
    engine.test_flush_readbacks();
    engine.test_flush_readbacks();
    let charcoal_scale = engine.cursor_preview_coverage_scale();
    assert!(
        charcoal_scale > 1.5,
        "charcoal scale precondition failed, got {charcoal_scale}"
    );

    engine.brush_load("Ink Pen").unwrap();
    engine.test_flush_readbacks();
    engine.test_flush_readbacks();
    let ink_pen_scale = engine.cursor_preview_coverage_scale();
    // The pivotal regression: a stale charcoal boost would carry over
    // and blow out ink pen. The compositor must recompute on brush
    // switch, dropping back to ≈1.0.
    assert!(
        (ink_pen_scale - 1.0).abs() < 0.05,
        "switching from charcoal to ink pen must drop the scale back \
         toward 1.0; got {ink_pen_scale} (previous charcoal scale was \
         {charcoal_scale})"
    );
}
