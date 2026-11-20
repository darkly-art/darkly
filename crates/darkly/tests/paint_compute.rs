//! The `paint` terminal as a compute pass: one dispatch per dab against a
//! packed `r32uint` ground that is the stroke scratch itself
//! (`crates/darkly/src/brush/nodes/paint.rs`).
//!
//! The accumulation laws are pinned exactly by `tests/brush_accumulation.rs`
//! and the pixels against the recorded dispatch spike by
//! `tests/paint_dispatch_spike.rs`; what is here is the framework plumbing
//! the port touches: the ground through the checkpoint ring's rewinds, the
//! `dispatches` counter, the selection under a cropped canvas, the ground
//! through a mid-stroke layer grow, and the one behaviour the per-dab
//! ceiling changes on purpose.
//!
//! Run with: `cargo test -p darkly --test paint_compute --features testing -- --test-threads=1`

use std::path::PathBuf;

use darkly::brush::builtin_brushes;
use darkly::brush::input_value::InputValue;
use darkly::brush::portable::PortableBrush;
use darkly::coord::CanvasRect;
use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::format::stroke_recording::{replay, ReplayPacing, StrokeRecording};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn test_engine(canvas: (u32, u32)) -> DarklyEngine {
    let (device, queue) = test_device();
    DarklyEngine::new(GpuContext::new_headless(device, queue), canvas.0, canvas.1)
}

fn fixture(name: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect()
}

/// The deterministic disc engine tests measure through, shared with
/// `brush_accumulation.rs`. The strokes here are compared byte for byte
/// across engines, and a shipped brush with a `random` node draws a fresh
/// stroke seed per engine, so the depositing strokes go through the fixture;
/// Smudge is installed where the live sampler is the point.
const ANALYTIC_DISC: &str = include_str!("fixtures/analytic_disc.yaml");

fn install_disc(engine: &mut DarklyEngine) {
    let portable: PortableBrush = serde_yaml_ng::from_str(ANALYTIC_DISC).expect("fixture parses");
    let brush = portable
        .into_brush(darkly::brush::registry(), "fixture")
        .expect("fixture builds");
    let json = serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph");
    engine
        .set_brush_graph(&json)
        .unwrap_or_else(|e| panic!("the disc fixture compiles: {e:?}"));
}

fn install_builtin(engine: &mut DarklyEngine, name: &str) {
    let brush = builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == name)
        .unwrap_or_else(|| panic!("{name} is a builtin"));
    let json = serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph");
    engine
        .set_brush_graph(&json)
        .unwrap_or_else(|e| panic!("{name} compiles: {e:?}"));
}

fn find_node_id(engine: &DarklyEngine, type_id: &str) -> String {
    engine
        .active_brush_graph()
        .nodes()
        .values()
        .find(|n| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in active graph"))
        .id
        .0
        .clone()
}

fn set_input(engine: &mut DarklyEngine, type_id: &str, port: &str, value: f32) {
    let id = find_node_id(engine, type_id);
    engine
        .brush_graph_set_input(&id, port, InputValue::Scalar(value))
        .unwrap_or_else(|e| panic!("{type_id}.{port}: {e:?}"));
}

fn event(x: f32, y: f32, t: f64) -> StrokeOp {
    StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: t,
    }
}

/// A straight stroke from `(x0, y)` to `(x1, y)` in `steps` events.
fn stroke_row(engine: &mut DarklyEngine, layer: LayerId, y: f32, x0: f32, x1: f32, steps: u32) {
    engine.begin_stroke(layer).unwrap();
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * (i as f32 / steps as f32);
        engine.stroke_to(event(x, y, i as f64 * 16.0));
    }
    engine.end_stroke();
    engine.test_flush_readbacks();
}

/// RGBA of the plane pixel `(x, y)` in a layer readback laid out over
/// `bounds`, or transparent off the layer.
fn plane_px(pixels: &[u8], bounds: CanvasRect, x: i32, y: i32) -> [u8; 4] {
    let lx = x - bounds.origin.x;
    let ly = y - bounds.origin.y;
    if lx < 0 || ly < 0 || lx >= bounds.width as i32 || ly >= bounds.height as i32 {
        return [0; 4];
    }
    let i = ((ly as u32 * bounds.width + lx as u32) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn layer_pixels(engine: &DarklyEngine, layer: LayerId) -> (Vec<u8>, CanvasRect) {
    let bounds = engine.layer_bounds(layer).expect("raster layer has bounds");
    (engine.test_readback_layer(layer), bounds)
}

/// The real stroke path: the recorded curvy stroke at full stabilisation,
/// so every event rewinds and restores through the checkpoint ring, whose
/// slots carry the `r32uint` ground as the scratch. Two fresh engines must
/// agree byte for byte, and the layer must hold paint.
#[test]
fn replay_is_deterministic_through_checkpoints_and_paints() {
    let recording =
        StrokeRecording::load(&fixture("recorded_curvy_stroke.json")).expect("fixture parses");
    // Under the headless device's downlevel limits, as in
    // `tests/stroke_replay.rs`.
    let canvas = (1024, 512);

    let run = || {
        let mut engine = test_engine(canvas);
        install_disc(&mut engine);
        set_input(&mut engine, "brush_settings", "stabilize", 1.0);
        let layer = engine.add_raster_layer(None);
        let timings = replay(
            &mut engine,
            &recording,
            layer,
            canvas,
            ReplayPacing::AsFastAsPossible,
            None,
        );
        assert_eq!(timings.len(), recording.events.len());
        engine.test_flush_readbacks();
        engine.test_readback_layer(layer)
    };
    let first = run();
    let second = run();
    assert!(
        first.as_chunks::<4>().0.iter().any(|px| px[3] > 0),
        "the replay left the layer empty"
    );
    assert!(first == second, "two replays of the same recording differ");
}

/// `dispatches` counts one per dab: what the bench's `dispatches/ev`
/// column reads for the compute terminal.
#[test]
fn dispatches_count_one_per_dab() {
    let mut engine = test_engine((256, 128));
    install_disc(&mut engine);
    let layer = engine.add_raster_layer(None);
    let _ = engine.drain_brush_perf_delta();
    stroke_row(&mut engine, layer, 64.0, 12.0, 244.0, 48);
    let perf = engine.drain_brush_perf_delta();
    assert!(perf.flushed_dabs > 0, "paint flushed no dabs");
    assert_eq!(
        perf.dispatches as u64, perf.flushed_dabs,
        "paint should issue one dispatch per dab"
    );
}

/// A brush that samples the live stroke refreshes the stroke's appearance
/// before every dab: two dispatches per dab, both counted.
#[test]
fn dispatches_count_two_per_dab_for_a_live_sampler() {
    let mut engine = test_engine((256, 128));
    install_disc(&mut engine);
    let layer = engine.add_raster_layer(None);
    stroke_row(&mut engine, layer, 64.0, 12.0, 244.0, 48);
    install_builtin(&mut engine, "Smudge");
    let _ = engine.drain_brush_perf_delta();
    stroke_row(&mut engine, layer, 60.0, 12.0, 244.0, 48);
    let perf = engine.drain_brush_perf_delta();
    assert!(perf.flushed_dabs > 0, "paint flushed no dabs");
    assert_eq!(
        perf.dispatches as u64,
        2 * perf.flushed_dabs,
        "a snapshot and a dab per dab"
    );
}

/// The Smudge over painted rows, through the recorded curvy stroke at
/// full stabilisation: every event rewinds through the checkpoint ring and
/// every dab's snapshot reads the restored grounds. Two fresh engines
/// agree byte for byte, and the smudge moved pigment.
#[test]
fn live_sampler_replay_is_deterministic_and_moves_pigment() {
    let recording =
        StrokeRecording::load(&fixture("recorded_curvy_stroke.json")).expect("fixture parses");
    let canvas = (1024, 512);
    let run = || {
        let mut engine = test_engine(canvas);
        install_disc(&mut engine);
        let layer = engine.add_raster_layer(None);
        for y in (16..canvas.1).step_by(32) {
            stroke_row(&mut engine, layer, y as f32, 8.0, canvas.0 as f32 - 8.0, 64);
        }
        let painted = layer_pixels(&engine, layer);
        install_builtin(&mut engine, "Smudge");
        set_input(&mut engine, "brush_settings", "stabilize", 1.0);
        let timings = replay(
            &mut engine,
            &recording,
            layer,
            canvas,
            ReplayPacing::AsFastAsPossible,
            None,
        );
        assert_eq!(timings.len(), recording.events.len());
        engine.test_flush_readbacks();
        (painted, layer_pixels(&engine, layer))
    };
    let (painted, first) = run();
    let (_, second) = run();
    assert!(first != painted, "the smudge moved no pigment");
    assert!(first == second, "two replays of the same recording differ");
}

/// The compute skeleton samples the selection per dab exactly as the
/// fragment skeleton did, through the window-anchored mask: pixels outside
/// a rectangular selection stay untouched and pixels inside are painted,
/// at the plane origin and after a crop that gives the canvas window a
/// non-zero origin over a layer that has grown to a negative offset.
#[test]
fn selection_masks_the_dab_pass_under_a_cropped_canvas() {
    let (w, h) = (128u32, 64u32);

    // At the plane origin: select the right half, stroke the whole width.
    {
        let mut engine = test_engine((w, h));
        install_disc(&mut engine);
        let layer = engine.add_raster_layer(None);
        engine.select_rect(
            64.0,
            0.0,
            64.0,
            h as f32,
            SelectionMode::Replace,
            false,
            0.0,
        );
        stroke_row(&mut engine, layer, 32.0, 4.0, 124.0, 48);
        let (px, bounds) = layer_pixels(&engine, layer);
        assert_eq!(
            plane_px(&px, bounds, 32, 32)[3],
            0,
            "an unselected pixel must stay transparent"
        );
        assert!(
            plane_px(&px, bounds, 96, 32)[3] > 0,
            "a selected pixel must be painted"
        );
    }

    // After a crop: grow the layer leftwards first so its offset is
    // negative, crop the window to a non-zero origin, select a band inside
    // the window, stroke across it.
    {
        let mut engine = test_engine((w, h));
        install_disc(&mut engine);
        let layer = engine.add_raster_layer(None);
        stroke_row(&mut engine, layer, 8.0, 20.0, -40.0, 16);
        let bounds = engine.layer_bounds(layer).expect("layer");
        assert!(
            bounds.origin.x < 0,
            "the stroke past the left edge must have grown the layer: {bounds:?}"
        );
        engine.resize_canvas(CanvasRect::from_xywh(16, 0, 96, h));
        engine.select_rect(
            64.0,
            0.0,
            48.0,
            h as f32,
            SelectionMode::Replace,
            false,
            0.0,
        );
        stroke_row(&mut engine, layer, 32.0, 20.0, 108.0, 48);
        let (px, bounds) = layer_pixels(&engine, layer);
        assert_eq!(
            plane_px(&px, bounds, 40, 32)[3],
            0,
            "an unselected pixel inside the window must stay transparent"
        );
        assert!(
            plane_px(&px, bounds, 90, 32)[3] > 0,
            "a selected pixel must be painted through the window's offset"
        );
    }
}

/// A stroke that starts on the layer and runs past its edge grows the
/// layer mid-stroke; the ground grows with it (its storage usage included),
/// so the pixels painted before the grow are still there afterwards and
/// the stroke is continuous across the old edge.
#[test]
fn stroke_that_grows_the_layer_keeps_its_ground() {
    let (w, h) = (128u32, 128u32);
    let mut engine = test_engine((w, h));
    install_disc(&mut engine);
    let layer = engine.add_raster_layer(None);
    let before = engine.layer_bounds(layer).expect("layer");
    stroke_row(&mut engine, layer, 64.0, 64.0, -60.0, 64);
    let (px, bounds) = layer_pixels(&engine, layer);
    assert!(
        bounds.origin.x < before.origin.x,
        "the stroke must have grown the layer leftwards: {before:?} -> {bounds:?}"
    );
    for x in [64, 32, 8, 0, -8, -30, -50] {
        assert!(
            plane_px(&px, bounds, x, 64)[3] > 0,
            "plane x = {x} on the stroke's row must be painted"
        );
    }
}

/// The per-dab wash law on a coloured pigment. For black the law is exact
/// (the stored premultiplied colour is exactly zero, and
/// `tests/brush_accumulation.rs` pins it bit for bit); for a coloured
/// pigment the stored colour is rounded to 8 bits, which perturbs the room
/// the ceiling finds by about one count, so a tight spacing that stacks
/// tens of dabs on a pixel may move it by one count against a loose one,
/// never more. The analytic disc at a 30x spacing spread, compared per
/// pixel in premultiplied space.
#[test]
fn coloured_wash_is_spacing_independent_within_two_lsb() {
    use darkly::brush::portable::PortableBrush;

    const ANALYTIC_DISC: &str = include_str!("fixtures/analytic_disc.yaml");
    const W: u32 = 256;
    const H: u32 = 128;
    const PIGMENT: [f32; 3] = [0.2, 0.5, 0.8];

    let run = |spacing: f32| -> Vec<u8> {
        let mut engine = test_engine((W, H));
        let yaml = ANALYTIC_DISC.replace(
            "  paint:\n    type: paint\n    inputs:\n",
            "  paint:\n    type: paint\n    inputs:\n      buildup: 0.0\n",
        );
        let portable: PortableBrush = serde_yaml_ng::from_str(&yaml).expect("fixture parses");
        let brush = portable
            .into_brush(darkly::brush::registry(), "fixture")
            .expect("fixture builds");
        let json = serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph");
        engine
            .set_brush_graph(&json)
            .unwrap_or_else(|e| panic!("fixture compiles: {e:?}"));
        set_input(&mut engine, "brush_settings", "spacing", spacing);
        let layer = engine.add_raster_layer(None);
        engine.set_foreground_color([PIGMENT[0], PIGMENT[1], PIGMENT[2], 1.0]);
        engine.begin_stroke(layer).unwrap();
        const SAMPLES: u32 = 40;
        for i in 0..SAMPLES {
            let x = 8.0 + i as f32 * ((W as f32 - 16.0) / SAMPLES as f32);
            engine.stroke_to(event(x, (H / 2) as f32, i as f64 * 16.0));
        }
        engine.end_stroke();
        engine.test_flush_readbacks();
        engine.test_readback_layer(layer)
    };

    let tight = run(0.01);
    let loose = run(0.30);
    let premul = |px: &[u8]| -> [u32; 4] {
        let a = px[3] as u32;
        [
            (px[0] as u32 * a + 127) / 255,
            (px[1] as u32 * a + 127) / 255,
            (px[2] as u32 * a + 127) / 255,
            a,
        ]
    };
    // The law's claim is about interior pixels, where every placement
    // deposits the disc's full coverage. At the disc's antialiased edge
    // the coverage a pixel sees varies with where each placement's rim
    // falls, and a tight spacing offers more placements to take the
    // maximum over, so the edge legitimately differs; `brush_accumulation`
    // reads the median for the same reason. Interior here is within two
    // counts of the stroke's peak alpha (the fixture deposits
    // `pressure * 0.1` per dab, so the peak is about 26), at both spacings.
    let peak = tight.chunks(4).map(|px| px[3]).max().unwrap_or(0);
    assert!(
        peak > 16,
        "the stroke must deposit a readable mark, peak {peak}"
    );
    let interior = |px: &[u8]| px[3] + 2 >= peak;
    let mut painted = 0;
    let mut max_diff = 0u32;
    for (t, l) in tight.chunks(4).zip(loose.chunks(4)) {
        if !interior(t) || !interior(l) {
            continue;
        }
        painted += 1;
        assert!(
            t[2] > t[0] && l[2] > l[0],
            "the pigment's chroma must survive the wash: {t:?} / {l:?}"
        );
        for (x, y) in premul(t).iter().zip(premul(l)) {
            max_diff = max_diff.max(x.abs_diff(y));
        }
    }
    assert!(
        painted > 1000,
        "the stroke painted only {painted} interior pixels"
    );
    assert!(
        max_diff <= 2,
        "a coloured wash must not depend on spacing beyond one count of colour rounding: \
         up to {max_diff} LSB apart between spacing 0.01 and 0.30"
    );
}
