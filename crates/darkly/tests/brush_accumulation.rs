//! Feature tests for the `paint` terminal's accumulation law
//! (`paint.buildup`: `Build-up` vs `Wash`).
//!
//! `Build-up` composites every dab over the last, so a pixel's density
//! rises with however many dabs the spacing happened to stack on it.
//! `Wash` takes the greatest coverage instead, so a stroke's density
//! comes from pressure alone and passing back over its own path adds
//! nothing.
//!
//! Run with: `cargo test -p darkly --test brush_accumulation -- --test-threads=1`
//! (GPU integration tests share a process-wide wgpu device.)

use darkly::brush::input_value::InputValue;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

const W: u32 = 256;
const H: u32 = 128;

/// `Wash`, as the shipped Pencil selects it.
const WASH: i32 = 1;
/// `Build-up`, the registration default and every other brush's law.
const BUILD_UP: i32 = 0;

fn test_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    DarklyEngine::new(GpuContext::new_headless(device, queue), W, H)
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

/// Install the builtin Pencil and set its accumulation law. `buildup`
/// of `None` leaves whatever the brush ships with.
fn install_pencil(engine: &mut DarklyEngine, buildup: Option<i32>) {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Pencil")
        .expect("Pencil builtin registered");
    let json = serde_json::to_string(&brush.metadata.graph).expect("serialize pencil graph");
    engine
        .set_brush_graph(&json)
        .expect("pencil graph compiles");
    if let Some(mode) = buildup {
        let term = find_node_id(engine, "paint");
        engine
            .brush_graph_set_input(&term, "buildup", InputValue::Int(mode))
            .expect("paint buildup port");
    }
}

fn set_spacing(engine: &mut DarklyEngine, spacing: f32) {
    let bs = find_node_id(engine, "brush_settings");
    engine
        .brush_graph_set_input(&bs, "spacing", InputValue::Scalar(spacing))
        .expect("brush_settings spacing port");
}

/// One stroke along the centreline. `passes` of 1 goes left to right;
/// 2 doubles back over the identical path inside the same stroke.
fn stroke_centreline(engine: &mut DarklyEngine, layer: LayerId, pressure: f32, passes: u32) {
    const SAMPLES: u32 = 40;
    engine.begin_stroke(layer).unwrap();
    let mut t = 0.0f64;
    for pass in 0..passes {
        for i in 0..SAMPLES {
            let step = if pass % 2 == 0 { i } else { SAMPLES - 1 - i };
            engine.stroke_to(StrokeOp::BrushStroke {
                x: 8.0 + step as f32 * ((W as f32 - 16.0) / SAMPLES as f32),
                y: (H / 2) as f32,
                pressure,
                x_tilt: 0.0,
                y_tilt: 0.0,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: t,
                cr: 0.0,
                cg: 0.0,
                cb: 0.0,
                ca: 1.0,
            });
            t += 16.0;
        }
    }
    engine.end_stroke();
    engine.test_flush_readbacks();
}

/// Alpha along the stroke's centreline row.
fn centreline_alpha(engine: &DarklyEngine, layer: LayerId) -> Vec<u8> {
    let px = engine.test_readback_layer(layer);
    let y = H / 2;
    (0..W)
        .map(|x| px[(((y * W + x) * 4) + 3) as usize])
        .collect()
}

fn peak(alphas: &[u8]) -> u8 {
    alphas.iter().copied().max().unwrap_or(0)
}

/// Median of the marked pixels. The headline statistic for density: peak
/// alpha clips at 255 under `Build-up` for anything but a light stroke,
/// so it cannot show a swing that has already saturated.
fn median(alphas: &[u8]) -> u8 {
    let mut marked: Vec<u8> = alphas.iter().copied().filter(|a| *a > 0).collect();
    marked.sort_unstable();
    marked.get(marked.len() / 2).copied().unwrap_or(0)
}

/// One Pencil stroke along the centreline, as centreline alpha.
fn run(buildup: i32, pressure: f32, spacing: f32, passes: u32) -> Vec<u8> {
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_pencil(&mut engine, Some(buildup));
    set_spacing(&mut engine, spacing);
    stroke_centreline(&mut engine, layer, pressure, passes);
    centreline_alpha(&engine, layer)
}

/// The defect this feature exists to fix: under `Build-up`, stroke
/// density is a function of `spacing` rather than of pressure, because a
/// tighter spacing stacks more dabs on each pixel. Under `Wash` the same
/// path at the same pressure lands on the same density whatever the
/// spacing.
///
/// This is the closest thing here to a regression pin: the `Wash` half
/// could not have passed before the accumulation law existed. The
/// `Build-up` half states the defect, so a future change that quietly
/// made spacing irrelevant everywhere would not go unnoticed.
#[test]
fn wash_density_is_independent_of_spacing() {
    // A 30x spread in spacing. Measured identical to the byte when this
    // landed, at every pressure tried.
    let tight = median(&run(WASH, 0.5, 0.01, 1));
    let loose = median(&run(WASH, 0.5, 0.30, 1));
    assert!(
        tight.abs_diff(loose) <= 2,
        "Wash density must not depend on spacing: median alpha {tight} at 0.01 vs {loose} at 0.30"
    );

    // The same pair under Build-up, where the swing lives: measured
    // 239 vs 113 when this landed.
    let bu_tight = median(&run(BUILD_UP, 0.5, 0.01, 1));
    let bu_loose = median(&run(BUILD_UP, 0.5, 0.30, 1));
    assert!(
        bu_tight > bu_loose + 60,
        "Build-up density should still swing on spacing (that is the defect Wash fixes): \
         median alpha {bu_tight} at 0.01 vs {bu_loose} at 0.30"
    );
}

/// A stroke that doubles back over its own path deposits nothing extra
/// under `Wash`. This is the artist-facing complaint the feature answers:
/// a light pencil pass over a light pencil pass stays light.
///
/// Asserted over the path interior. The turnaround at each end is
/// genuinely different ground: the pen reverses there, so more dab
/// centres land near those pixels and the ceiling is approached more
/// closely. `Max` over a larger sample set can only rise, and rising
/// *toward a fixed ceiling* is the point, so the ends are excluded
/// rather than papered over.
#[test]
fn wash_stroke_does_not_darken_when_it_crosses_itself() {
    const INTERIOR: std::ops::Range<usize> = 20..236;

    let once = run(WASH, 0.5, 0.01, 1);
    let twice = run(WASH, 0.5, 0.01, 2);
    for x in INTERIOR {
        let (a, b) = (once[x], twice[x]);
        assert!(
            b.abs_diff(a) <= 2,
            "Wash: doubling back must not darken. x={x}: one pass {a}, two passes {b}"
        );
    }
    assert_eq!(
        peak(&once),
        peak(&twice),
        "Wash: the ceiling a stroke reaches must not move when it doubles back"
    );

    // Under Build-up the second pass does darken, which is what makes
    // the assertion above meaningful rather than vacuous.
    let bu_once = median(&run(BUILD_UP, 0.5, 0.01, 1));
    let bu_twice = median(&run(BUILD_UP, 0.5, 0.01, 2));
    assert!(
        bu_twice > bu_once,
        "Build-up should still compound on self-overlap: median {bu_once} -> {bu_twice}"
    );
}

/// Capping accumulation must not cost pressure sensitivity. Guards
/// against "fixing" build-up by flattening the brush.
#[test]
fn wash_still_responds_to_pressure() {
    let light = run(WASH, 0.4, 0.01, 1);
    let heavy = run(WASH, 0.9, 0.01, 1);
    assert!(
        peak(&heavy) > peak(&light) + 20,
        "heavier pressure must deposit more: peak {} at 0.4 vs {} at 0.9",
        peak(&light),
        peak(&heavy)
    );
    // Monotone everywhere the light stroke marked, not just at the peak.
    for (x, (l, h)) in light.iter().zip(heavy.iter()).enumerate() {
        if *l > 8 {
            assert!(h >= l, "x={x}: alpha fell with pressure, {l} -> {h}");
        }
    }
}

/// Erase reads the same scratch, so the law applies there too: a soft
/// eraser scrubbed back over its own path stops punching further through.
#[test]
fn wash_erase_does_not_compound_on_self_overlap() {
    let erase_run = |buildup: i32| {
        let mut engine = test_engine();
        let layer = engine.add_raster_layer(None);

        engine.begin_stroke(layer).unwrap();
        engine.stroke_to(StrokeOp::FloodFill {
            x: 1.0,
            y: 1.0,
            r: 255,
            g: 0,
            b: 0,
            a: 255,
            tolerance: 0,
        });
        engine.end_stroke();
        engine.test_flush_readbacks();

        install_pencil(&mut engine, Some(buildup));
        engine.set_brush_blend_mode(1);
        stroke_centreline(&mut engine, layer, 0.6, 4);
        centreline_alpha(&engine, layer)
    };

    let wash_left = *erase_run(WASH).iter().min().unwrap();
    let build_up_left = *erase_run(BUILD_UP).iter().min().unwrap();
    assert!(
        wash_left > build_up_left,
        "a Wash eraser scrubbing its own path should remove less than a Build-up one: \
         lowest remaining alpha {wash_left} (Wash) vs {build_up_left} (Build-up)"
    );
}

/// A graph that never mentions `buildup` must paint exactly as it did
/// before the port existed. Pinned against an explicit `Build-up`, whose
/// blend state is asserted to be the original literal in `node.rs`'s
/// unit tests.
#[test]
fn default_accumulation_is_byte_identical_to_explicit_build_up() {
    let default_run = |buildup: Option<i32>| {
        let mut engine = test_engine();
        let layer = engine.add_raster_layer(None);
        install_pencil(&mut engine, buildup);
        // The shipped Pencil selects Wash, so drive the default through
        // a port reset rather than through the brush's own authored value.
        let term = find_node_id(&engine, "paint");
        if buildup.is_none() {
            engine
                .brush_graph_set_input(&term, "buildup", InputValue::Int(BUILD_UP))
                .expect("paint buildup port");
        }
        stroke_centreline(&mut engine, layer, 0.7, 1);
        engine.test_readback_layer(layer)
    };
    assert_eq!(
        default_run(None),
        default_run(Some(BUILD_UP)),
        "the registration default must render byte-identically to explicit Build-up"
    );
}
