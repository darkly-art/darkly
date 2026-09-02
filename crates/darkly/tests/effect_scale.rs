//! The effect resolution scale — one global knob, both sides of the divider.
//!
//! An effect can render below native resolution and be scaled back up, trading
//! sharpness for fill rate. This file is about that trade: that the scale is
//! honoured wherever the effect sits, that it composes with an effect's own
//! declared cost, that changing it actually takes effect, and that the round
//! trip does not tint what it resamples.
//!
//! Run with: `cargo test -p darkly --test effect_scale --features testing -- --test-threads=1`

use darkly::config::ConfigValue;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

const SCALE_KEY: &str = "rendering.effect_scale";

fn set_scale(value: f64) {
    darkly::config::set(SCALE_KEY, ConfigValue::Float(value));
}

/// Restores the scale on drop. `config` is a thread-local store and the GPU
/// tests share one thread, so a leaked override would leak sideways into every
/// test that runs after this one.
struct ScaleGuard(f64);

impl ScaleGuard {
    fn set(value: f64) -> Self {
        let previous = darkly::config::get_f64(SCALE_KEY);
        set_scale(value);
        ScaleGuard(previous)
    }
}

impl Drop for ScaleGuard {
    fn drop(&mut self) {
        set_scale(self.0);
    }
}

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn fill_layer(engine: &mut DarklyEngine, layer_id: LayerId, r: u8, g: u8, b: u8) {
    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r,
        g,
        b,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
    engine.render(0.0);
}

/// A single opaque dab, leaving the rest of the layer transparent — the alpha
/// edge the resampling assertions need.
fn paint_dot(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32, color: [f32; 3]) {
    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: color[0],
        cg: color[1],
        cb: color[2],
        ca: 1.0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
    engine.render(0.0);
}

fn effect(engine: &mut DarklyEngine, pipeline: &str) -> LayerId {
    let defaults: Vec<_> = engine
        .filter_param_defs(pipeline)
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect();
    engine
        .add_filter_layer(pipeline, defaults, None)
        .unwrap_or_else(|| panic!("`{pipeline}` should be addable as an effect layer"))
}

/// A canvas-space effect over a filled raster, composited once so its instance
/// is realized.
fn realized_canvas_effect(engine: &mut DarklyEngine, pipeline: &str) -> LayerId {
    let base = engine.add_raster_layer(None);
    fill_layer(engine, base, 128, 128, 128);
    let fx = effect(engine, pipeline);
    engine.test_flush_readbacks();
    let _ = engine.test_readback_canvas();
    fx
}

// ---------------------------------------------------------------------------
// The scale applies in both spaces
// ---------------------------------------------------------------------------

/// The knob reaches canvas space at all — the half of the divider that used to
/// hardcode full resolution.
#[test]
fn canvas_space_effect_renders_at_the_configured_scale() {
    let _guard = ScaleGuard::set(0.5);
    let mut engine = test_engine(64, 64);
    let fx = realized_canvas_effect(&mut engine, "invert");

    assert_eq!(
        engine.test_effect_reduced_size(fx),
        Some((32, 32)),
        "a canvas-space effect renders at the configured fraction of the canvas"
    );
}

/// One knob, one answer, whichever side of the line the effect is on. This is
/// what pins the two spaces to a single mechanism rather than two.
#[test]
fn both_spaces_render_at_the_same_scale() {
    let _guard = ScaleGuard::set(0.5);
    let mut engine = test_engine(64, 64);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);
    let canvas_fx = effect(&mut engine, "invert");
    let screen_fx = effect(&mut engine, "grain");
    engine.set_screen_space_boundary(1);

    // Realize both: the canvas instance through a composite, the screen one
    // through the run, which a headless engine only sizes on demand.
    let _ = engine.test_readback_canvas();
    let _ = engine.test_readback_screen_run(64, 64);

    assert_eq!(
        engine.test_effect_reduced_size(canvas_fx),
        Some((32, 32)),
        "canvas space"
    );
    assert_eq!(
        engine.test_effect_reduced_size(screen_fx),
        Some((32, 32)),
        "screen space"
    );
}

/// At full scale the effect reads and writes the caller's own pair, with no
/// intermediate textures and no extra passes. That path must stay free.
#[test]
fn a_scale_of_one_skips_the_reduced_path() {
    let _guard = ScaleGuard::set(1.0);
    let mut engine = test_engine(64, 64);
    let fx = realized_canvas_effect(&mut engine, "invert");

    assert_eq!(
        engine.test_effect_reduced_size(fx),
        None,
        "full scale must not allocate a reduced pair"
    );
}

/// The global scale and an effect's own declared cost multiply — neither
/// overrides the other. `painting` declares 0.7, so at 0.5 it lands on 0.35.
#[test]
fn per_effect_factor_composes_with_the_configured_scale() {
    let _guard = ScaleGuard::set(0.5);
    let mut engine = test_engine(100, 100);
    let fx = realized_canvas_effect(&mut engine, "painting");

    assert_eq!(
        engine.test_effect_reduced_size(fx),
        Some((35, 35)),
        "0.5 × 0.7 = 0.35 of a 100px canvas"
    );
}

// ---------------------------------------------------------------------------
// What the round trip does to pixels
// ---------------------------------------------------------------------------

/// The reduced path is really in the canvas encode, not merely prepared: a
/// scaled invert cannot be the exact per-pixel inverse the full-scale one is.
#[test]
fn a_reduced_canvas_effect_actually_resamples_the_composite() {
    let mut engine = test_engine(64, 64);
    let base = engine.add_raster_layer(None);
    // A hard edge, so resampling has something to soften.
    fill_layer(&mut engine, base, 255, 0, 0);
    paint_dot(&mut engine, base, 20.0, 20.0, [0.0, 0.0, 1.0]);
    let _fx = effect(&mut engine, "invert");

    let _guard = ScaleGuard::set(1.0);
    let exact = engine.test_readback_canvas();

    set_scale(0.5);
    let reduced = engine.test_readback_canvas();

    assert_ne!(
        exact, reduced,
        "a reduced-resolution invert must differ from the exact per-pixel one"
    );
}

/// Accumulators hold straight alpha, so an unweighted resample drags colour
/// toward the black that transparent texels carry — a dark rim around every
/// silhouette, baked into the export. Colour must be weighted by coverage in
/// both directions of the round trip.
#[test]
fn a_reduced_canvas_effect_does_not_darken_transparent_edges() {
    let _guard = ScaleGuard::set(0.5);
    let mut engine = test_engine(64, 64);
    let base = engine.add_raster_layer(None);
    // One opaque red dab on an otherwise empty layer.
    paint_dot(&mut engine, base, 32.0, 32.0, [1.0, 0.0, 0.0]);
    let _fx = effect(&mut engine, "invert");

    let pixels = engine.test_readback_canvas();
    let mut worst = 0u8;
    for quad in pixels.as_chunks::<4>().0 {
        // Inverting red gives cyan, so any covered texel should have a low red
        // channel. A texel pulled toward black before the invert comes back
        // with red raised instead.
        if quad[3] > 8 {
            worst = worst.max(quad[0]);
        }
    }

    // Weighted, this is 0; unweighted it reaches 60, so the threshold sits
    // well clear of both.
    assert!(
        worst < 16,
        "covered texels must stay cyan through the reduced round trip; \
         worst red channel was {worst}, which means colour was averaged \
         against transparent black"
    );
}

// ---------------------------------------------------------------------------
// Change detection
// ---------------------------------------------------------------------------

/// Regression: the realized scale was never part of an instance's fingerprint.
/// `structural_match` compares `pipeline_id`, `space`, `render_size` and the
/// `targets` revision — and for a canvas instance `render_size` is the parent
/// accumulator, which a scale change does not move. So a composite could run
/// after the change and still reuse the instance built at the old scale.
#[test]
fn changing_the_scale_rebuilds_a_canvas_instance() {
    let _guard = ScaleGuard::set(1.0);
    let mut engine = test_engine(64, 64);
    let fx = realized_canvas_effect(&mut engine, "invert");
    assert_eq!(
        engine.test_effect_reduced_size(fx),
        None,
        "at full scale the effect runs on the accumulator directly"
    );

    set_scale(0.5);

    // Dirty the composite the ordinary way, so the frame genuinely reaches
    // `sync_effect_instances` — this test is about the fingerprint, not about
    // whether anything woke the pipeline up.
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 10, 20, 30);
    let _ = engine.test_readback_canvas();

    assert_eq!(
        engine.test_effect_reduced_size(fx),
        Some((32, 32)),
        "a composite after a scale change must rebuild the instance at the new scale"
    );
}

/// Regression: nothing on the canvas path polled the scale at all. The only
/// poll lived on the screen run and ran from `Compositor::render`, which a
/// headless engine never reaches; `render_offscreen` returns early while the
/// composite is clean, so a scale change alone could not wake anything.
#[test]
fn a_scale_change_alone_wakes_the_canvas() {
    let _guard = ScaleGuard::set(1.0);
    let mut engine = test_engine(64, 64);
    let fx = realized_canvas_effect(&mut engine, "invert");
    assert_eq!(engine.test_effect_reduced_size(fx), None);

    set_scale(0.5);

    // No mutation whatsoever — the config change is the only event.
    let _ = engine.test_readback_canvas();

    assert_eq!(
        engine.test_effect_reduced_size(fx),
        Some((32, 32)),
        "a scale change with nothing else dirty must still reach the rebuild"
    );
}

/// The scale poll now runs on every frame, ahead of the dirty gate. It must
/// report drift only when there is drift — otherwise it marks the compositor
/// dirty forever and the frame loop never idles.
#[test]
fn a_steady_frame_does_not_rebuild_or_redirty() {
    let _guard = ScaleGuard::set(0.5);
    let mut engine = test_engine(64, 64);
    let _fx = realized_canvas_effect(&mut engine, "invert");

    let rebuilds = engine.test_effect_rebuilds();
    assert!(
        !engine.test_render_offscreen(),
        "with nothing changed the compositor must already be clean"
    );
    assert!(
        !engine.test_render_offscreen(),
        "and must stay clean on the frame after that"
    );
    assert_eq!(
        engine.test_effect_rebuilds(),
        rebuilds,
        "a steady frame must not rebuild any effect instance"
    );
}

/// An animated effect's clock lives on the effect itself, so a rebuild that
/// went back to the registry would rewind it. Dragging the scale slider must
/// not restart every veil in the document.
#[test]
fn a_rebuild_preserves_an_animated_effect_clock() {
    let _guard = ScaleGuard::set(1.0);
    let mut engine = test_engine(64, 64);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);
    let _fx = effect(&mut engine, "grain");
    let _ = engine.test_readback_canvas();

    // Advance the clock, and snapshot what that looks like.
    engine.test_tick_animations(1.0);
    for i in 1..=8 {
        engine.test_tick_animations(1.0 + i as f32 * 0.05);
    }
    let advanced = engine.test_readback_canvas();

    // Rebuild by changing the scale, then put it back so the comparison is
    // between two full-scale composites and only the rebuild differs.
    set_scale(0.5);
    let _ = engine.test_readback_canvas();
    set_scale(1.0);
    let after_rebuild = engine.test_readback_canvas();

    assert_eq!(
        advanced, after_rebuild,
        "a rebuild must carry the effect's animation state across, not reset it"
    );
}
