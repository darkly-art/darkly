//! Pressure settle: the first and last moments of a stroke are the pen
//! landing and leaving, so each end of a stroke holds the strongest pressure
//! read between it and its boundary, at every stabilize strength.
//!
//! Run with: `cargo test -p darkly --test stroke_settle --features testing -- --test-threads=1`

mod common;

use common::{assert_same, headless, new_engine, readback, ConfigGuard};
use darkly::coord::CanvasRect;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::layer::LayerId;

const CANVAS: (u32, u32) = (512, 128);
const SETTLE: &str = "input.pressureSettleTime";
const PREDICTION: &str = "input.predictionHorizon";

/// Events are this far apart in x, and this far apart in time.
const STEP_PX: f32 = 2.0;
const STEP_MS: f64 = 1000.0 / 240.0;
/// Events in the let-up, and their spacing in time: 54 ms in all, so the
/// 50 ms settle time cuts between two events rather than on one, and the
/// last kept event is the comet's last.
const LET_UP: usize = 12;
const LET_UP_STEP_MS: f64 = 4.5;

const STRENGTHS: [f32; 3] = [0.0, 0.4, 1.0];

enum Lift {
    /// Pressure falls from 1 to 0 over the let-up.
    LetUp,
    /// Pressure holds at 1 to the end.
    Held,
}

/// How a test stroke is drawn.
#[derive(Clone, Copy, Debug)]
enum Drawn {
    /// Every event, then the pen up, with no frame between.
    Headless,
    /// A frame after every event, every rendered vertex re-rendered
    /// whenever it changes at all.
    FramePerEvent,
}

fn op(x: f32, pressure: f32, t: f64) -> StrokeOp {
    StrokeOp::BrushStroke {
        x,
        y: 64.0,
        pressure,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: t,
    }
}

fn x_at(i: usize) -> f32 {
    40.0 + i as f32 * STEP_PX
}

/// A comet drawn light to heavy: `n` events with pressure rising 0.1 to 1,
/// then [`LET_UP`] more still travelling, their pressure set by `lift`.
fn comet(n: usize, lift: Lift) -> Vec<StrokeOp> {
    let rising = (0..n).map(|i| {
        op(
            x_at(i),
            0.1 + 0.9 * i as f32 / (n - 1) as f32,
            i as f64 * STEP_MS,
        )
    });
    let last = (n - 1) as f64 * STEP_MS;
    let lifting = (1..=LET_UP).map(|k| {
        let pressure = match lift {
            Lift::LetUp => 1.0 - k as f32 / LET_UP as f32,
            Lift::Held => 1.0,
        };
        op(x_at(n - 1 + k), pressure, last + k as f64 * LET_UP_STEP_MS)
    });
    rising.chain(lifting).collect()
}

/// 80 events at full pressure, but for a dip to 0.5 over events 40 to 60,
/// well after the settle time, so a rule that flattened the whole stroke
/// would show. `ramp` opens it with a touch-down ramp from 0.1, reaching
/// full pressure at event 10 (42 ms, inside the settle time).
fn pressed(ramp: bool) -> Vec<StrokeOp> {
    (0..80)
        .map(|i| {
            let pressure = if ramp && i < 10 {
                0.1 + 0.09 * i as f32
            } else if (40..=60).contains(&i) {
                1.0 - 0.5 * (std::f32::consts::PI * (i - 40) as f32 / 20.0).sin()
            } else {
                1.0
            };
            op(x_at(i), pressure, i as f64 * STEP_MS)
        })
        .collect()
}

/// A hard tap: 9 events at one point 4.5 ms apart (36 ms), the pressure
/// rising to 0.9 and falling back.
fn tap() -> Vec<StrokeOp> {
    [0.1, 0.3, 0.6, 0.85, 0.9, 0.8, 0.5, 0.25, 0.1]
        .into_iter()
        .enumerate()
        .map(|(i, pressure)| op(200.0, pressure, i as f64 * 4.5))
        .collect()
}

fn engine(stabilize: f32) -> (DarklyEngine, LayerId) {
    let mut engine = new_engine(CANVAS, "Ink Pen", stabilize);
    engine.set_stroke_seed(Some(7));
    let layer = engine.add_raster_layer(None);
    (engine, layer)
}

/// `ops` as one stroke with a frame after every event, every rendered
/// vertex re-rendered whenever it changes at all.
fn frame_per_event(engine: &mut DarklyEngine, layer: LayerId, ops: &[StrokeOp]) {
    engine.test_set_exact_divergence();
    engine.begin_stroke(layer).unwrap();
    for op in ops {
        engine.stroke_to(*op);
        engine.render(0.0);
    }
    engine.end_stroke();
}

/// `ops` drawn at `stabilize` as `drawn`, with prediction off.
fn draw(stabilize: f32, ops: &[StrokeOp], drawn: Drawn) -> (Vec<u8>, CanvasRect) {
    let _prediction = ConfigGuard::set(PREDICTION, 0.0);
    let (mut engine, layer) = engine(stabilize);
    match drawn {
        Drawn::Headless => headless(&mut engine, layer, ops),
        Drawn::FramePerEvent => frame_per_event(&mut engine, layer, ops),
    }
    readback(&mut engine, layer)
}

/// `ops` painted headless at `stabilize` with prediction off.
fn paint(stabilize: f32, ops: &[StrokeOp]) -> (Vec<u8>, CanvasRect) {
    draw(stabilize, ops, Drawn::Headless)
}

/// [`paint`] with the settle time off.
fn paint_unsettled(stabilize: f32, ops: &[StrokeOp]) -> (Vec<u8>, CanvasRect) {
    let _off = ConfigGuard::set(SETTLE, 0.0);
    paint(stabilize, ops)
}

/// How many pixels tall the stroke is at canvas column `x`.
fn height_at((pixels, bounds): &(Vec<u8>, CanvasRect), x: f32) -> usize {
    let column = (x as i32 - bounds.origin.x) as u32;
    (0..bounds.height)
        .filter(|&y| pixels[((y * bounds.width + column) * 4 + 3) as usize] > 128)
        .count()
}

/// Regression: a stroke started with a touch-down ramp came out thin at its
/// start, the low first readings drawn as read and, with a stabilizer,
/// carried forward from the pinned first vertex. The start takes the
/// strongest reading up to the settle time, so it renders exactly as if the
/// pen had pressed fully from the first event, and the rest as drawn.
#[test]
fn a_touch_down_ramp_keeps_the_start() {
    for stabilize in STRENGTHS {
        let held = paint_unsettled(stabilize, &pressed(false));
        for drawn in [Drawn::Headless, Drawn::FramePerEvent] {
            let what = format!("stabilize {stabilize}, {drawn:?}");
            let ramped = draw(stabilize, &pressed(true), drawn);
            let start = x_at(3);
            println!(
                "{what}: start height ramped {} held {}",
                height_at(&ramped, start),
                height_at(&held, start)
            );
            assert_same(&what, "ramped", ramped, "held", held.clone());
        }
    }
}

/// Regression: a hard stationary tap drew a single dab at the touch-down
/// reading, however hard the pen pressed. A tap shorter than the settle time
/// takes its peak, so it is a dot as large as one dab at the peak.
#[test]
fn a_hard_tap_is_a_full_dot() {
    for stabilize in STRENGTHS {
        let peak = paint(stabilize, &[op(200.0, 0.9, 0.0)]);
        for drawn in [Drawn::Headless, Drawn::FramePerEvent] {
            let what = format!("stabilize {stabilize}, {drawn:?}");
            let dot = draw(stabilize, &tap(), drawn);
            println!(
                "{what}: dot height {} peak dab {}",
                height_at(&dot, 200.0),
                height_at(&peak, 200.0)
            );
            assert_same(&what, "tap", dot, "one dab at the peak", peak.clone());
        }
    }
}

/// A comet drawn light to heavy came out thin at its head when the pen let
/// up slowly. The end takes the strongest reading since its boundary, so a
/// let-up renders exactly as if the pen had held its pressure to the end,
/// with the head the held pen's own.
#[test]
fn a_slow_let_up_keeps_the_comet_head() {
    for stabilize in STRENGTHS {
        // 150 and 151, so the let-up starts both between the resampler's
        // vertices and on one.
        for n in [150, 151] {
            let what = format!("stabilize {stabilize}, {n} events");
            let let_up = paint(stabilize, &comet(n, Lift::LetUp));
            let held = paint(stabilize, &comet(n, Lift::Held));
            let reference = paint_unsettled(stabilize, &comet(n, Lift::Held));
            // Just behind where the let-up starts.
            let head = x_at(n - 1) - 10.0;
            let (height, reference_height) = (height_at(&let_up, head), height_at(&reference, head));
            assert_same(&what, "let up", let_up, "held", held);
            assert_eq!(
                height, reference_height,
                "{what}: the head is {height} px tall, the held pen's {reference_height}"
            );
        }
    }
}

/// The let-up comet drawn live with a frame per event, the same stroke
/// headless, and a stroke-edit redraw of the live one are pixel-identical,
/// and the redraws of its start and end rewind to a checkpoint rather than
/// falling back to redrawing the whole stroke.
#[test]
fn live_headless_and_stroke_edit_agree() {
    let _prediction = ConfigGuard::set(PREDICTION, 0.0);
    for stabilize in STRENGTHS {
        let what = format!("stabilize {stabilize}");
        let ops = comet(150, Lift::LetUp);
        let (mut live, live_layer) = engine(stabilize);
        frame_per_event(&mut live, live_layer, &ops);
        assert_eq!(
            live.test_stroke_full_rerender_events(),
            0,
            "{what}: a redraw found no checkpoint"
        );
        let drawn = readback(&mut live, live_layer);
        let (mut whole, whole_layer) = engine(stabilize);
        headless(&mut whole, whole_layer, &ops);
        assert_same(
            &what,
            "live",
            drawn.clone(),
            "headless",
            readback(&mut whole, whole_layer),
        );
        assert!(live.stroke_edit_begin());
        assert_same(
            &what,
            "live",
            drawn,
            "edited",
            readback(&mut live, live_layer),
        );
    }
}

/// A long curving stroke whose pressure rises at touch-down, swells, and
/// lets up slowly, drawn with a frame every few events: settling its start
/// and its end rewrites further back than the stabilizer alone moves, and
/// both rewinds still find a checkpoint, with prediction on or off.
#[test]
fn the_pen_up_rewind_finds_a_checkpoint() {
    for (stabilize, horizon) in [(0.0f32, 0.0), (0.4, 10.0), (1.0, 30.0), (0.6, 0.0)] {
        let _prediction = ConfigGuard::set(PREDICTION, horizon);
        let mut engine = new_engine((1024, 1024), "Ink Pen", stabilize);
        engine.set_stroke_seed(Some(7));
        let layer = engine.add_raster_layer(None);
        engine.begin_stroke(layer).unwrap();
        let n = 900;
        for i in 0..n {
            let th = i as f32 * 0.01;
            let r = 380.0 * (1.0 - th * 0.05);
            let left = (n - i) as f32;
            let pressure = if left < 30.0 {
                left / 30.0
            } else {
                0.5 + 0.4 * (th * 3.0).sin()
            };
            engine.stroke_to(StrokeOp::BrushStroke {
                x: 512.0 + r * th.cos(),
                y: 512.0 + r * th.sin(),
                pressure,
                x_tilt: 0.0,
                y_tilt: 0.0,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: i as f64 * STEP_MS,
            });
            if i % 4 == 0 {
                engine.render(0.0);
            }
        }
        engine.end_stroke();
        assert_eq!(
            engine.test_stroke_full_rerender_events(),
            0,
            "stabilize {stabilize}, prediction {horizon} ms: a redraw found no checkpoint"
        );
    }
}

/// A stroke painted with no frame between its events is stabilized once,
/// settled or not: the pen is up before anything is stabilized.
#[test]
fn a_headless_stroke_relaxes_once() {
    let ops = comet(150, Lift::LetUp);
    let updates: Vec<u64> = [50.0, 0.0]
        .into_iter()
        .map(|settle| {
            let _settle = ConfigGuard::set(SETTLE, settle);
            let (mut engine, layer) = engine(0.6);
            let before = darkly::brush::stabilizers::laplacian::relaxed_vertex_updates();
            headless(&mut engine, layer, &ops);
            darkly::brush::stabilizers::laplacian::relaxed_vertex_updates() - before
        })
        .collect();
    assert_eq!(updates[0], updates[1]);
}

/// With the settle time at 0, the touch-down ramp thins the start and the
/// let-up thins the end, as drawn.
#[test]
fn a_settle_time_of_zero_keeps_both_ramps() {
    for stabilize in STRENGTHS {
        assert_ne!(
            paint_unsettled(stabilize, &comet(150, Lift::LetUp)),
            paint_unsettled(stabilize, &comet(150, Lift::Held)),
            "stabilize {stabilize}: the let-up"
        );
        assert_ne!(
            paint_unsettled(stabilize, &pressed(true)),
            paint_unsettled(stabilize, &pressed(false)),
            "stabilize {stabilize}: the touch-down ramp"
        );
    }
}

/// An unstabilized stroke rewinds only to settle its ends: while its start
/// is open and at pen-up, never in between.
#[test]
fn an_unstabilized_stroke_rewinds_only_for_its_ends() {
    let _prediction = ConfigGuard::set(PREDICTION, 0.0);
    let ops = comet(150, Lift::LetUp);
    let (mut engine, layer) = engine(0.0);
    engine.test_set_exact_divergence();
    engine.begin_stroke(layer).unwrap();
    // Every event of the comet's first 50 ms raises its start.
    let opening = ops.iter().take_while(|op| match op {
        StrokeOp::BrushStroke { time_ms, .. } => *time_ms < 50.0,
        _ => false,
    });
    let opening = opening.count();
    let mut rewinds = Vec::new();
    for op in &ops {
        engine.stroke_to(*op);
        engine.render(0.0);
        rewinds.push(engine.test_stroke_rewinds());
    }
    let settled = rewinds[opening];
    assert!(settled > 0, "the start never rewound");
    assert_eq!(
        rewinds.last(),
        Some(&settled),
        "the stroke rewound after its start settled"
    );
    engine.end_stroke();
    assert!(engine.test_stroke_rewinds() > settled, "the pen-up never rewound");
    assert_eq!(engine.test_stroke_full_rerender_events(), 0);
}
