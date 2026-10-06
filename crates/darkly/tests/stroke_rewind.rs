//! The checkpoint ring's region save and restore, pinned against a
//! from-scratch render.
//!
//! A stabilized stroke rewinds to a checkpoint, restores it, and replays
//! the dabs after it, at every frame where a rendered vertex moved. The ring
//! copies only the region those dabs dirtied, so a region that is one texel
//! too small leaves a stale dab or a hole. The oracle is the engine's own
//! full re-render path: `test_set_full_rerender` clears the ring before
//! every rewind, so each frame renders the polyline from index 0 with the
//! terminal's whole-scratch prologue. That path and the incremental one must
//! produce the same bytes.
//!
//! The cells stabilize at [`STABILIZE`] with a divergence epsilon of zero.
//! An unstabilized stroke never rewinds, so it would compare the oracle with
//! itself. With the production epsilon, a vertex that drifts by less than
//! it keeps the dabs it was first rendered with, while the oracle redraws
//! it where it now lies, so the two would differ regardless of the ring. At
//! zero every moved vertex is re-rendered, and vertices the stabilizer has
//! not moved are bit-identical in both runs.
//!
//! The same comparison pins flush cadence: a stroke rendered whole at
//! pen-up, as a headless embedder that runs no frame during it gets, matches
//! the same stroke rendered a frame at a time.
//!
//! Run with: `cargo test -p darkly --test stroke_rewind --features testing -- --test-threads=1`

use std::path::PathBuf;

use darkly::brush::builtin_brushes;
use darkly::brush::gpu_context::MAX_DABS_PER_PHASE;
use darkly::brush::input_value::InputValue;
use darkly::brush::resampler::MAX_COMMITS_PER_SAMPLE;
use darkly::brush::stabilizers::laplacian::relaxed_vertex_updates;
use darkly::coord::CanvasRect;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::format::stroke_recording::{replay, EventTiming, ReplayPacing, StrokeRecording};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

/// Stabilizer strength every cell paints at: deep enough that a frame
/// rewinds across several checkpoint segments.
const STABILIZE: f32 = 0.5;

fn fixture(name: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect()
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

fn set_input(engine: &mut DarklyEngine, type_id: &str, port: &str, value: f32) {
    let id = engine
        .active_brush_graph()
        .nodes()
        .values()
        .find(|n| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in active graph"))
        .id
        .0
        .clone();
    engine
        .brush_graph_set_input(&id, port, InputValue::Scalar(value))
        .unwrap_or_else(|e| panic!("{type_id}.{port}: {e:?}"));
}

/// A headless engine with `brush` installed at the given stabilizer
/// strength.
fn new_engine(canvas: (u32, u32), brush: &str, stabilize: f32) -> DarklyEngine {
    let (device, queue) = test_device();
    let mut engine = DarklyEngine::new(GpuContext::new_headless(device, queue), canvas.0, canvas.1);
    install_builtin(&mut engine, brush);
    set_input(&mut engine, "brush_settings", "stabilize", stabilize);
    engine
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
        cr: 0.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    }
}

/// `ops` as one stroke with no frame between them, as a headless embedder
/// paints: the pen-up flush renders it whole.
fn headless(engine: &mut DarklyEngine, layer: LayerId, ops: &[StrokeOp]) {
    engine.begin_stroke(layer).unwrap();
    for op in ops {
        engine.stroke_to(*op);
    }
    engine.end_stroke();
}

/// `ops` as one stroke with a frame after every event.
fn frame_per_event(engine: &mut DarklyEngine, layer: LayerId, ops: &[StrokeOp]) {
    engine.begin_stroke(layer).unwrap();
    for op in ops {
        engine.stroke_to(*op);
        engine.render(0.0);
    }
    engine.end_stroke();
}

/// The layer's pixels and bounds once every pending GPU readback has landed.
fn readback(engine: &mut DarklyEngine, layer: LayerId) -> (Vec<u8>, CanvasRect) {
    engine.test_flush_readbacks();
    let bounds = engine.layer_bounds(layer).expect("raster layer has bounds");
    (engine.test_readback_layer(layer), bounds)
}

/// Two runs' layers, compared byte for byte. `what` names the configuration
/// in the failure message. Returns the layer bounds for the caller's
/// structural assertions.
fn assert_same(
    what: &str,
    a_name: &str,
    (a, bounds): (Vec<u8>, CanvasRect),
    b_name: &str,
    (b, b_bounds): (Vec<u8>, CanvasRect),
) -> CanvasRect {
    assert_eq!(
        bounds, b_bounds,
        "{a_name} and {b_name} grew the layer differently ({what})"
    );
    assert!(
        b.as_chunks::<4>().0.iter().any(|px| px[3] > 0),
        "the stroke left the layer empty ({what})"
    );
    if a != b {
        let differing: Vec<(i32, i32, [u8; 4], [u8; 4])> = a
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| {
                let x = bounds.origin.x + (i as u32 % bounds.width) as i32;
                let y = bounds.origin.y + (i as u32 / bounds.width) as i32;
                (x, y, *a, *b)
            })
            .collect();
        panic!(
            "{a_name} differs from {b_name} in {} of {} pixels ({what}, layer {bounds:?}); \
             first few (x, y, {a_name}, {b_name}): {:?}",
            differing.len(),
            b.len() / 4,
            &differing[..differing.len().min(8)]
        );
    }
    bounds
}

/// One stroke configuration, run either incrementally (the ring's region
/// copies) or through the forced full re-render (the oracle).
struct Cell<'a> {
    brush: &'a str,
    /// `paint.buildup`; `Some` adds the `build` storage channel, so the
    /// ring snapshots and restores two grounds.
    buildup: Option<f32>,
    canvas: (u32, u32),
    /// Canvas window to crop to before the stroke, giving the window a
    /// non-zero plane origin.
    crop: Option<CanvasRect>,
}

impl Cell<'_> {
    /// The cell's engine at [`STABILIZE`] and zero divergence epsilon, with
    /// a fresh raster layer and the window cropped, before any stroke.
    fn engine(&self) -> (DarklyEngine, LayerId) {
        let mut engine = new_engine(self.canvas, self.brush, STABILIZE);
        engine.test_set_divergence_epsilon(0.0);
        if let Some(b) = self.buildup {
            set_input(&mut engine, "paint", "buildup", b);
        }
        let layer = engine.add_raster_layer(None);
        if let Some(window) = self.crop {
            engine.resize_canvas(window);
        }
        (engine, layer)
    }

    fn run(
        &self,
        full_rerender: bool,
        drive: impl FnOnce(&mut DarklyEngine, LayerId),
    ) -> (Vec<u8>, CanvasRect) {
        let (mut engine, layer) = self.engine();
        engine.test_set_full_rerender(full_rerender);
        drive(&mut engine, layer);
        if !full_rerender {
            assert!(
                engine.test_stroke_rewinds() > 0,
                "the incremental run never rewound, so this comparison would \
                 be the oracle against itself"
            );
            assert_eq!(
                engine.test_stroke_full_rerender_events(),
                0,
                "the incremental run fell back to a full re-render, so this \
                 comparison would be the oracle against itself"
            );
        }
        readback(&mut engine, layer)
    }

    /// Both runs of `drive`, compared byte for byte. Returns the layer
    /// bounds for the caller's structural assertions.
    fn assert_matches_oracle(&self, drive: impl Fn(&mut DarklyEngine, LayerId)) -> CanvasRect {
        assert_same(
            &format!("{}, buildup {:?}", self.brush, self.buildup),
            "incremental rewind",
            self.run(false, &drive),
            "the full re-render",
            self.run(true, &drive),
        )
    }
}

fn replay_recording(
    engine: &mut DarklyEngine,
    layer: LayerId,
    canvas: (u32, u32),
) -> Vec<EventTiming> {
    let recording =
        StrokeRecording::load(&fixture("recorded_curvy_stroke.json")).expect("fixture parses");
    let timings = replay(
        engine,
        &recording,
        layer,
        canvas,
        ReplayPacing::AsFastAsPossible,
        None,
    );
    assert_eq!(timings.len(), recording.events.len());
    timings
}

/// The recorded curvy stroke as ops, scaled to `canvas`.
fn recorded_ops(canvas: (u32, u32)) -> Vec<StrokeOp> {
    let recording =
        StrokeRecording::load(&fixture("recorded_curvy_stroke.json")).expect("fixture parses");
    let scale = (
        canvas.0 as f32 / recording.canvas_width as f32,
        canvas.1 as f32 / recording.canvas_height as f32,
    );
    recording
        .events
        .iter()
        .map(|e| e.to_stroke_op(scale))
        .collect()
}

/// A checkpoint save never submits on its own: it is recorded into the
/// submission of the segment it snapshots. A frame is the stroke prologue
/// or the rewind, one submission per segment, which also flushes the
/// segment's dabs, an unsaved tail, and the commit, so it submits at most
/// three more times than it flushes dabs. A save in a submission of its own
/// adds one per segment. Checked per frame rather than as a stroke total,
/// which could not tell one frame over from another under.
#[test]
fn checkpoint_saves_share_their_segment_submission() {
    let canvas = (1024, 512);
    let mut engine = new_engine(canvas, "Ink Pen", STABILIZE);
    let layer = engine.add_raster_layer(None);
    let timings = replay_recording(&mut engine, layer, canvas);
    engine.test_flush_readbacks();
    assert!(engine.test_stroke_rewinds() > 0, "the stroke must rewind");
    assert_eq!(engine.test_stroke_full_rerender_events(), 0);
    let over: Vec<(usize, u32, u32)> = timings
        .iter()
        .filter(|t| t.submits > t.dab_flushes + 3)
        .map(|t| (t.index, t.submits, t.dab_flushes))
        .collect();
    assert!(
        over.is_empty(),
        "frames with more submissions than dab flushes + rewind + tail + commit \
         (index, submits, dab flushes): {over:?}"
    );
}

/// A stroke that walks inside the window, jumps off its left and top edges
/// in one event (growing the layer to a negative origin), walks on
/// outside, reverses, and ends with a 90 px jump back inside, so a
/// rewind's dirtied region lands wholly outside the restored slot's frame.
///
/// Every edge crossing is a single jump longer than the dab's reach. A dab
/// whose footprint merely crosses the layer edge is clipped by design
/// (`StrokeOp::required_coverage` grows the layer only once a dab centre
/// escapes it) and is re-rendered unclipped only if a later rewind reaches
/// it; that history is not something a from-scratch render can reproduce,
/// and it is not the ring's doing.
fn grow_reverse_jump(engine: &mut DarklyEngine, layer: LayerId) {
    engine.begin_stroke(layer).unwrap();
    let mut t = 0.0;
    // A frame per event, so the stroke rewinds as it goes.
    let mut go = |x: f32, y: f32| {
        engine.stroke_to(event(x, y, t));
        engine.render(0.0);
        t += 16.0;
    };
    let (mut x, mut y) = (150.0, 120.0);
    for _ in 0..5 {
        go(x, y);
        x -= 12.0;
        y -= 10.0;
    }
    (x, y) = (-40.0, -40.0);
    go(x, y);
    for _ in 0..4 {
        x -= 12.0;
        y -= 10.0;
        go(x, y);
    }
    for _ in 0..6 {
        x += 12.0;
        y += 10.0;
        go(x, y);
    }
    go(x + 90.0, y + 90.0);
    engine.end_stroke();
}

/// The recorded curvy stroke (204 events with reversals) at the downlevel
/// limit: slots are reused across indices, outgrown and reallocated, and
/// the stroke crosses frame boundaries many times.
#[test]
fn recorded_stroke_rewinds_match_full_rerender() {
    // Under the headless device's downlevel limits, as in
    // `tests/stroke_replay.rs`.
    let canvas = (1024, 512);
    let cell = Cell {
        brush: "Ink Pen",
        buildup: None,
        canvas,
        crop: None,
    };
    cell.assert_matches_oracle(|engine, layer| {
        replay_recording(engine, layer, canvas);
    });
}

/// Black vertical bars across the layer in the Ink Pen, then `brush`
/// back at [`STABILIZE`]: something for a brush that moves existing
/// pigment to move. The bars stay a dab's reach inside the layer: pigment
/// at the border would let the smear cross the edge at a dab clipped
/// before the layer grows, the history `grow_reverse_jump`'s note says a
/// from-scratch render cannot reproduce.
fn lay_stripes(engine: &mut DarklyEngine, layer: LayerId, canvas: (u32, u32), brush: &str) {
    install_builtin(engine, "Ink Pen");
    set_input(engine, "brush_settings", "stabilize", 0.0);
    const INSET: u32 = 64;
    for x in (INSET..canvas.0 - INSET).step_by(48) {
        engine.begin_stroke(layer).unwrap();
        for i in 0..=8 {
            let y = INSET as f32 + (canvas.1 - 2 * INSET) as f32 * i as f32 / 8.0;
            engine.stroke_to(event(x as f32, y, i as f64 * 16.0));
        }
        engine.end_stroke();
    }
    install_builtin(engine, brush);
    set_input(engine, "brush_settings", "stabilize", STABILIZE);
}

/// The recorded stroke through the Smudge over a striped layer: every
/// rewind restores the grounds and the next dab's appearance snapshot
/// reads them back, and the appearance mirror itself is never
/// checkpointed, so a mirror texel a dab read without refreshing it shows
/// here as a difference from the full re-render.
#[test]
fn live_sampler_rewinds_match_full_rerender() {
    let canvas = (1024, 512);
    let cell = Cell {
        brush: "Smudge",
        buildup: None,
        canvas,
        crop: None,
    };
    cell.assert_matches_oracle(|engine, layer| {
        lay_stripes(engine, layer, canvas, "Smudge");
        replay_recording(engine, layer, canvas);
    });
}

/// The same stroke with the `build` channel declared: the ring snapshots
/// and restores two `r32uint` grounds with one set of rects, and the
/// commit reads both, so a channel the restore missed shows in the layer.
#[test]
fn two_grounds_rewind_together() {
    let canvas = (1024, 512);
    let cell = Cell {
        brush: "Ink Pen",
        buildup: Some(0.5),
        canvas,
        crop: None,
    };
    cell.assert_matches_oracle(|engine, layer| {
        replay_recording(engine, layer, canvas);
    });
}

/// A cropped window with a non-zero plane origin, a mid-stroke layer grow
/// to a negative origin, a reversal and a jump: slots saved under the old
/// extent are restored under the new one, and the first save after the
/// grow reallocates the slot. One ground and two.
#[test]
fn growth_and_crop_rewind_match_full_rerender() {
    for buildup in [None, Some(0.5)] {
        let cell = Cell {
            brush: "Ink Pen",
            buildup,
            canvas: (256, 192),
            crop: Some(CanvasRect::from_xywh(16, 8, 224, 176)),
        };
        let bounds = cell.assert_matches_oracle(grow_reverse_jump);
        assert!(
            bounds.origin.x < 0 && bounds.origin.y < 0,
            "the stroke must have grown the layer past the plane origin: {bounds:?}"
        );
    }
}

/// A boustrophedon across the canvas in 8 px steps, 16 px in from its edges.
fn zig_zag_ops(canvas: (u32, u32)) -> Vec<StrokeOp> {
    let mut ops = Vec::new();
    let (x0, x1) = (16.0, canvas.0 as f32 - 16.0);
    let mut y = 16.0;
    let mut forward = true;
    while y <= canvas.1 as f32 - 16.0 {
        let mut x = if forward { x0 } else { x1 };
        while (x0..=x1).contains(&x) {
            ops.push(event(x, y, ops.len() as f64 * 4.0));
            x += if forward { 8.0 } else { -8.0 };
        }
        forward = !forward;
        y += 8.0;
    }
    ops
}

/// A small Ink Pen at the 1 px spacing floor over a long zig-zag, with no
/// frame during the stroke, places more dabs in the pen-up flush's one
/// segment than one phase holds. The phase splits at the cap, and the layer
/// matches the same stroke rendered a frame per event, whose phases stay far
/// below it. Unstabilized, so the stroke never rewinds and lays no
/// checkpoint.
#[test]
fn headless_stroke_splits_phases_at_the_dab_cap() {
    let canvas = (1024, 256);
    let ops = zig_zag_ops(canvas);
    let run = |drive: fn(&mut DarklyEngine, LayerId, &[StrokeOp])| {
        let mut engine = new_engine(canvas, "Ink Pen", 0.0);
        set_input(&mut engine, "brush_settings", "size", 0.02);
        set_input(&mut engine, "brush_settings", "spacing", 0.0);
        let layer = engine.add_raster_layer(None);
        drive(&mut engine, layer, &ops);
        let dabs = engine.test_stroke_total_dabs();
        let submits = engine.drain_brush_perf_delta().submits as u64;
        (readback(&mut engine, layer), dabs, submits)
    };
    let (headless_layer, dabs, submits) = run(headless);
    assert!(
        dabs > MAX_DABS_PER_PHASE as u64,
        "the stroke must place more dabs than one phase holds: {dabs}"
    );
    // The prologue, one per full phase, the tail and the commit. Ink Pen's
    // paint terminal writes its uniform ring once per dab flush, so no ring
    // flush adds a submission.
    assert!(
        submits <= 3 + dabs / MAX_DABS_PER_PHASE as u64,
        "{submits} submissions for {dabs} dabs"
    );
    let (framed_layer, _, _) = run(frame_per_event);
    assert_same(
        "Ink Pen at stabilize 0",
        "headless",
        headless_layer,
        "a frame per event",
        framed_layer,
    );
}

/// A stroke no frame ran during is rendered whole by the pen-up flush, and
/// nothing can diverge after pen-up, so that flush saves no checkpoint: the
/// prologue, one segment and the commit, plus one submission per full dab
/// phase. A checkpoint every few vertices would add a submission each.
#[test]
fn pen_up_flush_saves_no_checkpoints() {
    let canvas = (1024, 512);
    let mut engine = new_engine(canvas, "Ink Pen", STABILIZE);
    let layer = engine.add_raster_layer(None);
    headless(&mut engine, layer, &recorded_ops(canvas));
    let dabs = engine.test_stroke_total_dabs();
    let submits = engine.drain_brush_perf_delta().submits as u64;
    assert_eq!(
        engine.test_stroke_rewinds(),
        0,
        "the stroke's only flush has nothing rendered before it to rewind"
    );
    assert!(
        submits <= 3 + dabs / MAX_DABS_PER_PHASE as u64,
        "{submits} submissions for {dabs} dabs"
    );
}

/// A frame before every event leaves the last one for the pen-up flush. At
/// a divergence epsilon of zero that sample moves every vertex in the
/// stabilizer's window, so the flush rewinds; it then renders to the tip as
/// one segment and commits, saving nothing.
#[test]
fn live_pen_up_flush_renders_one_segment() {
    let canvas = (1024, 512);
    let cell = Cell {
        brush: "Ink Pen",
        buildup: None,
        canvas,
        crop: None,
    };
    let (mut engine, layer) = cell.engine();
    engine.begin_stroke(layer).unwrap();
    for op in recorded_ops(canvas) {
        engine.render(0.0);
        engine.stroke_to(op);
    }
    let rewinds = engine.test_stroke_rewinds();
    engine.drain_brush_perf_delta();
    engine.end_stroke();
    let submits = engine.drain_brush_perf_delta().submits;
    assert_eq!(
        engine.test_stroke_rewinds(),
        rewinds + 1,
        "the pen-up flush must rewind"
    );
    assert!(
        submits <= 3,
        "the pen-up flush submitted {submits} times: more than its rewind, one segment and the commit"
    );
}

/// The recorded stroke rendered whole at pen-up matches it rendered a flush
/// per event with its rewinds and checkpoint restores: skipping the pen-up
/// flush's checkpoints and rendering it as one segment changes no pixel.
#[test]
fn headless_stroke_matches_frame_by_frame() {
    let canvas = (1024, 512);
    let cell = Cell {
        brush: "Ink Pen",
        buildup: None,
        canvas,
        crop: None,
    };
    let framed = cell.run(false, |engine, layer| {
        replay_recording(engine, layer, canvas);
    });
    let (mut engine, layer) = cell.engine();
    headless(&mut engine, layer, &recorded_ops(canvas));
    assert_same(
        "Ink Pen",
        "headless",
        readback(&mut engine, layer),
        "a flush per event",
        framed,
    );
}

/// Regression: a stroke no frame ran during reaches the stabilizer as one
/// batch at pen-up, which the Laplacian relaxes once. Fed a sample at a time,
/// it relaxed a window per resampled vertex: at stabilize 0.6 that was most
/// of a headless stroke's CPU.
#[test]
fn headless_stroke_relaxes_once() {
    let canvas = (1024, 512);
    let ops = recorded_ops(canvas);
    let mut engine = new_engine(canvas, "Ink Pen", 0.6);
    let layer = engine.add_raster_layer(None);
    let before = relaxed_vertex_updates();
    headless(&mut engine, layer, &ops);
    let updates = relaxed_vertex_updates() - before;
    // One relaxation of ceil(0.6^2 x 160) = 58 sweeps over every resampled
    // vertex, of which a raw sample commits at most `MAX_COMMITS_PER_SAMPLE`.
    let bound = 58 * (ops.len() * MAX_COMMITS_PER_SAMPLE + 1) as u64;
    assert!(
        updates <= bound,
        "the stroke relaxed {updates} vertex updates, more than one pass ({bound})"
    );
}
