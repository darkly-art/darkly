//! Feature tests for the `paint` terminal's accumulation law
//! (`paint.buildup`: `Build-up` vs `Wash`).
//!
//! `Build-up` composites every dab over the last, so a pixel's density
//! rises with however many dabs the spacing happened to stack on it.
//! `Wash` takes the greatest coverage instead, so a stroke's density
//! comes from pressure alone and passing back over its own path adds
//! nothing.
//!
//! The same control governs both places overlapping deposit accumulates: dabs
//! within a stroke (a hardware blend on the scratch) and strokes against the
//! layer (an alpha cap at the commit). They are different mechanisms because a
//! per-dab softened law would put tone back under the spacing slider, but they
//! are tuned to one behaviour: overlap does not compound, wherever it happens.
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

/// Wash: the bottom of the accumulation dial. A pixel takes its strongest
/// dab and the commit refuses anything past what one pass would deposit.
const WASH: f32 = 0.0;
/// Build-up: the top of the dial, the registration default, and the law
/// every shipped brush but the Pencil paints under. Every dab composites
/// over the last.
const BUILD_UP: f32 = 1.0;

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

/// Install a builtin brush by name, exactly as its YAML declares it, through
/// the JSON path the app uses.
fn install_builtin(engine: &mut DarklyEngine, name: &str) {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == name)
        .unwrap_or_else(|| panic!("{name} builtin registered"));
    let json = serde_json::to_string(&brush.metadata.graph).expect("serialize brush graph");
    engine
        .set_brush_graph(&json)
        .unwrap_or_else(|e| panic!("{name} graph compiles: {e:?}"));
}

/// Install the builtin Pencil and set its accumulation law. `buildup`
/// of `None` leaves whatever the brush ships with.
fn install_pencil(engine: &mut DarklyEngine, buildup: Option<f32>) {
    install_builtin(engine, "Pencil");
    if let Some(dial) = buildup {
        set_buildup(engine, dial);
    }
}

/// Set the accumulation dial on the active graph's `paint` terminal.
fn set_buildup(engine: &mut DarklyEngine, dial: f32) {
    let term = find_node_id(engine, "paint");
    engine
        .brush_graph_set_input(&term, "buildup", InputValue::Scalar(dial))
        .expect("paint buildup port");
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

/// One stroke of a builtin brush along the centreline, as centreline alpha.
/// `buildup` of `None` takes the brush's law as authored.
fn run_brush(
    name: &str,
    buildup: Option<f32>,
    pressure: f32,
    spacing: f32,
    passes: u32,
) -> Vec<u8> {
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_builtin(&mut engine, name);
    if let Some(dial) = buildup {
        set_buildup(&mut engine, dial);
    }
    set_spacing(&mut engine, spacing);
    stroke_centreline(&mut engine, layer, pressure, passes);
    centreline_alpha(&engine, layer)
}

/// One Pencil stroke along the centreline under an explicit law.
fn run(buildup: f32, pressure: f32, spacing: f32, passes: u32) -> Vec<u8> {
    run_brush("Pencil", Some(buildup), pressure, spacing, passes)
}

// ── Byte-identity pins ──────────────────────────────────────────────────

const INK_PEN_PIN: [u8; 256] = [
    253, 253, 254, 254, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 254, 254,
    253, 253, 253, 252, 248, 248, 248, 238, 232,
];

/// One stroke of a builtin exactly as its YAML declares it, as centreline
/// alpha: the shape every pin below compares.
fn authored_centreline(name: &str) -> Vec<u8> {
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_builtin(&mut engine, name);
    stroke_centreline(&mut engine, layer, 0.7, 1);
    centreline_alpha(&engine, layer)
}

/// The Ink Pen renders byte-identically to a row captured on the unchanged
/// tree, before any of the dial work.
///
/// It is the brush that never mentions `buildup`, so it is the one the dial
/// owes exact preservation: graph, per-dab blend and commit all have to come
/// out where they were. Its dab is analytic rather than noise-textured, so
/// the row holds on every adapter.
///
/// The Pencil carries no such pin. It is the brush the dial was built for
/// and is tuned as a design choice rather than held fixed, so a byte row
/// would pin the tuning rather than guard a behaviour. What the Pencil owes
/// is stated as behaviour instead, throughout this file.
#[test]
fn ink_pen_renders_byte_identically_to_its_pin() {
    assert_eq!(
        authored_centreline("Ink Pen"),
        INK_PEN_PIN,
        "a brush that never mentions the accumulation port must not move"
    );
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
    // A 30x spread in spacing. Not bit-identical, and not expected to be:
    // the Pencil's tip texture is dab-space noise, so each dab draws an
    // independent sample and the max over N of them creeps up with N, which
    // spacing sets. Measured at 3/255 across this spread, against the 126/255
    // the same pair swings under Build-up below.
    let tight = median(&run(WASH, 0.5, 0.01, 1));
    let loose = median(&run(WASH, 0.5, 0.30, 1));
    assert!(
        tight.abs_diff(loose) <= 6,
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
    // Same dab-space-grain caveat as the spacing test: the return leg samples
    // the tip texture at different dab-local offsets, so the peak can creep by
    // a count. Measured at 1/255.
    assert!(
        peak(&once).abs_diff(peak(&twice)) <= 2,
        "Wash: the ceiling a stroke reaches must not move when it doubles back: {} -> {}",
        peak(&once),
        peak(&twice)
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
    let erase_run = |buildup: f32| {
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
    let run = |buildup: Option<f32>| {
        let mut engine = test_engine();
        let layer = engine.add_raster_layer(None);
        // The Ink Pen never mentions the port, so its "as authored" render
        // is the registration default itself, not a value the test wrote.
        install_builtin(&mut engine, "Ink Pen");
        if let Some(dial) = buildup {
            set_buildup(&mut engine, dial);
        }
        stroke_centreline(&mut engine, layer, 0.7, 1);
        engine.test_readback_layer(layer)
    };
    assert_eq!(
        run(None),
        run(Some(BUILD_UP)),
        "the registration default must render byte-identically to explicit Build-up"
    );
}

// ── Cross-stroke coverage ceiling (the commit) ──────────────────────────

/// Fill `layer` with an opaque colour.
fn fill_opaque(engine: &mut DarklyEngine, layer: LayerId, r: u8, g: u8, b: u8) {
    engine.begin_stroke(layer).unwrap();
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
}

/// Under `Wash`, separate strokes along the same path at the same pressure do
/// not compound: coverage stops at what one pass would have deposited.
///
/// The equality here is **exact**, not approximate, and that is a real
/// property rather than a lucky tolerance. `stroke_seed` reaches `random`
/// nodes only, never `noise`, which seeds from its own compile-time port, so
/// a repeated pass draws the identical field and `max` of a value with itself
/// is that value.
#[test]
fn wash_caps_coverage_across_strokes() {
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_pencil(&mut engine, Some(WASH));

    stroke_centreline(&mut engine, layer, 0.7, 1);
    let after_one = centreline_alpha(&engine, layer);

    for n in 2..=8 {
        stroke_centreline(&mut engine, layer, 0.7, 1);
        let now = centreline_alpha(&engine, layer);
        assert_eq!(
            now, after_one,
            "Wash: stroke {n} along the same path at the same pressure must change nothing"
        );
    }

    // Build-up compounds to opaque over the same eight strokes, which is what
    // makes the assertion above meaningful rather than vacuous.
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_pencil(&mut engine, Some(BUILD_UP));
    for _ in 0..8 {
        stroke_centreline(&mut engine, layer, 0.7, 1);
    }
    assert_eq!(
        peak(&centreline_alpha(&engine, layer)),
        255,
        "Build-up should still compound to opaque across strokes"
    );
}

/// The ceiling saturates per pigment, not globally: a pass of a *different*
/// colour still deposits over an existing mark.
///
/// This is what separates the deposit model from a plain alpha cap. Room is
/// measured as distance to the pigment being laid down, so a red mark is
/// nowhere near saturated for blue and takes it normally, while a second red
/// pass on the same mark has no room and does nothing.
#[test]
fn wash_saturates_per_pigment_not_globally() {
    let coloured = |engine: &mut DarklyEngine, layer: LayerId, rgb: [f32; 3]| {
        engine.begin_stroke(layer).unwrap();
        for i in 0..40 {
            engine.stroke_to(StrokeOp::BrushStroke {
                x: 8.0 + i as f32 * ((W as f32 - 16.0) / 40.0),
                y: (H / 2) as f32,
                pressure: 0.7,
                x_tilt: 0.0,
                y_tilt: 0.2,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: i as f64 * 16.0,
                cr: rgb[0],
                cg: rgb[1],
                cb: rgb[2],
                ca: 1.0,
            });
        }
        engine.end_stroke();
        engine.test_flush_readbacks();
    };

    // Same pigment twice: the second pass finds no room.
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_pencil(&mut engine, Some(WASH));
    coloured(&mut engine, layer, [1.0, 0.0, 0.0]);
    let once = engine.test_readback_layer(layer);
    coloured(&mut engine, layer, [1.0, 0.0, 0.0]);
    assert_eq!(
        engine.test_readback_layer(layer),
        once,
        "a second pass of the same pigment must find no room"
    );

    // A different pigment is a different axis, and deposits.
    coloured(&mut engine, layer, [0.0, 0.0, 1.0]);
    let after_blue = engine.test_readback_layer(layer);
    let moved = (0..(W * H) as usize)
        .filter(|i| after_blue[i * 4 + 2] > once[i * 4 + 2].saturating_add(8))
        .count();
    assert!(
        moved > 64,
        "blue over a red mark must still deposit; only {moved} pixels gained blue"
    );
}

/// The layer's transparency must not change the result. A mark made on a
/// transparent layer over white paper and the same mark made directly onto an
/// opaque white layer have to look the same, at any stroke count.
///
/// This is the property the whole deposit model exists for. Room is read from
/// the pixel's current colour composited over the deposit's origin, which is
/// the one quantity both representations agree on, so neither the alpha cap
/// this replaced (inert on opaque ground) nor a luminance heuristic (wrong for
/// light pigments) is involved.
#[test]
fn wash_behaves_the_same_on_transparent_and_opaque_layers() {
    /// Appearance over white, so the two representations are comparable.
    fn over_white(engine: &DarklyEngine, layer: LayerId) -> Vec<[f32; 3]> {
        let px = engine.test_readback_layer(layer);
        let y = H / 2;
        (0..W)
            .map(|x| {
                let i = ((y * W + x) * 4) as usize;
                let a = px[i + 3] as f32 / 255.0;
                [0, 1, 2].map(|c| (px[i + c] as f32 / 255.0) * a + (1.0 - a))
            })
            .collect()
    }

    for strokes in [1, 2, 4] {
        let mut transparent = test_engine();
        let t_layer = transparent.add_raster_layer(None);
        install_pencil(&mut transparent, Some(WASH));

        let mut opaque = test_engine();
        let o_layer = opaque.add_raster_layer(None);
        fill_opaque(&mut opaque, o_layer, 255, 255, 255);
        install_pencil(&mut opaque, Some(WASH));

        for _ in 0..strokes {
            stroke_centreline(&mut transparent, t_layer, 0.7, 1);
            stroke_centreline(&mut opaque, o_layer, 0.7, 1);
        }

        for (x, (t, o)) in over_white(&transparent, t_layer)
            .iter()
            .zip(over_white(&opaque, o_layer).iter())
            .enumerate()
        {
            for c in 0..3 {
                let diff = ((t[c] - o[c]).abs() * 255.0).round() as i32;
                assert!(
                    diff <= 1,
                    "{strokes} stroke(s), x={x}, channel {c}: transparent {} vs opaque {} \
                     (diff {diff}/255); layer transparency must not change the result",
                    t[c],
                    o[c]
                );
            }
        }
    }
}

/// Mask targets are R8: the commit writes only `.r`, which comes from the
/// colour path the ceiling never touches, and the alpha it caps is discarded.
///
/// The case worth pinning is the subtractive one. Painting black into a
/// revealed mask must still pull it down; a design that capped colour along
/// with coverage would make mask refinement impossible, since a mask's
/// broadcast alpha is always 1.
#[test]
fn wash_does_not_break_subtractive_mask_painting() {
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    engine.add_mask(layer).expect("add mask");
    let mask_id = engine.host_mask_id(layer).expect("mask filter id");

    install_pencil(&mut engine, Some(WASH));
    let before = engine.test_readback_mask(layer);
    stroke_centreline(&mut engine, mask_id, 0.9, 1);
    let after = engine.test_readback_mask(layer);

    let y = H / 2;
    let lowered = (0..W as usize)
        .filter(|x| after[y as usize * W as usize + x] < before[y as usize * W as usize + x])
        .count();
    assert!(
        lowered > 32,
        "painting black into a revealed mask under Wash must still pull it down; \
         only {lowered} pixels fell"
    );
}

/// The ceiling is set by the pass, so a heavier pass raises it. Guards against
/// "fixing" accumulation by making every later stroke a no-op.
#[test]
fn a_heavier_pass_still_darkens_through_the_ceiling() {
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_pencil(&mut engine, Some(WASH));

    stroke_centreline(&mut engine, layer, 0.7, 1);
    let light = peak(&centreline_alpha(&engine, layer));
    stroke_centreline(&mut engine, layer, 0.95, 1);
    let heavy = peak(&centreline_alpha(&engine, layer));

    assert!(
        heavy > light + 40,
        "a heavier pass must raise the ceiling: peak {light} -> {heavy}"
    );
}

// ── The accumulation dial ───────────────────────────────────────────────

/// Darkest centreline pixel composited over white. Lower is darker. Works
/// for transparent and opaque destinations alike, which is the point.
fn darkest_over_white(engine: &DarklyEngine, layer: LayerId) -> i32 {
    let px = engine.test_readback_layer(layer);
    let y = H / 2;
    (0..W)
        .map(|x| {
            let i = ((y * W + x) * 4) as usize;
            let a = px[i + 3] as f32 / 255.0;
            (((px[i] as f32 / 255.0) * a + (1.0 - a)) * 255.0) as i32
        })
        .min()
        .unwrap()
}

/// The ladder every dial test walks.
const DIAL: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// How far the two overlap sites may drift apart before they count as
/// disagreeing: the dab-space grain the file already allows per site (two
/// counts each, as in the spacing and self-overlap tests), not a number read
/// off a measurement.
const SITE_PARITY_TOL: i32 = 4;

/// Spacing the dial ladders walk at. Deliberately looser than any brush
/// ships with: at tight spacing the stacking half saturates early and the
/// ladder flattens, which would make a monotonicity test pass for the wrong
/// reason.
const LADDER_SPACING: f32 = 0.30;

/// Spacing the shipped Pencil authors, near enough. A claim about the brush
/// a painter is handed is measured where that brush works.
const PENCIL_SPACING: f32 = 0.01;

/// What retracing a path inside one stroke adds, at `dial`. `None` takes the
/// brush's law as authored.
fn within_stroke_surcharge(dial: Option<f32>, spacing: f32) -> i32 {
    let once = median(&run_brush("Pencil", dial, 0.5, spacing, 1)) as i32;
    let twice = median(&run_brush("Pencil", dial, 0.5, spacing, 2)) as i32;
    twice - once
}

/// What a second, separate stroke over the same path adds, at `dial`.
fn across_stroke_surcharge(dial: Option<f32>, spacing: f32) -> i32 {
    let mut engine = test_engine();
    let layer = engine.add_raster_layer(None);
    install_pencil(&mut engine, dial);
    set_spacing(&mut engine, spacing);
    stroke_centreline(&mut engine, layer, 0.5, 1);
    let once = median(&centreline_alpha(&engine, layer)) as i32;
    stroke_centreline(&mut engine, layer, 0.5, 1);
    let twice = median(&centreline_alpha(&engine, layer)) as i32;
    twice - once
}

/// Retracing a path inside one stroke builds more as the dial rises, and
/// nothing at all at the bottom.
///
/// Measured surcharge in median alpha at [`LADDER_SPACING`], pressure 0.5:
/// 2 / 25 / 43 / 54 / 62 across the ladder.
#[test]
fn within_stroke_overlap_builds_more_as_the_dial_rises() {
    let ladder: Vec<i32> = DIAL
        .iter()
        .map(|d| within_stroke_surcharge(Some(*d), LADDER_SPACING))
        .collect();
    assert!(
        ladder[0] <= 2,
        "at Wash a retrace must add nothing: {ladder:?}"
    );
    for pair in ladder.windows(2) {
        assert!(
            pair[1] > pair[0],
            "each step of the dial must build more than the one below: {ladder:?}"
        );
    }
    assert!(
        ladder[4] >= 8,
        "at Build-up a retrace must visibly darken: {ladder:?}"
    );
}

/// A second, separate stroke builds the same way a retrace does, at every
/// setting. This is the property the dial is for: the two places overlap
/// can happen agree, rather than one compounding while the other refuses.
///
/// Measured gap between the two sites: 2 / 1 / 1 / 2 / 1 counts, against
/// the [`SITE_PARITY_TOL`] the file allows.
#[test]
fn both_overlap_sites_build_alike_at_every_dial_setting() {
    for dial in DIAL {
        let within = within_stroke_surcharge(Some(dial), LADDER_SPACING);
        let across = across_stroke_surcharge(Some(dial), LADDER_SPACING);
        assert!(
            (within - across).abs() <= SITE_PARITY_TOL,
            "dial {dial}: a retrace added {within} but a second stroke added {across};              the two sites must build alike"
        );
    }
}

/// A pass on fresh ground gets darker or stays as the dial rises, never
/// lighter, and the dial's whole range is reachable.
///
/// This is the honest statement of what the dial costs: the two ends
/// deposit different amounts on untouched ground (Wash takes one dab,
/// Build-up stacks every dab that lands), so a mix of them lands between.
/// Measured darkest-over-white after one stroke at pressure 0.7:
/// 149 / 74 / 30 / 9 / 1. A brush author holds it level by lowering
/// `build_flow` against `wash_flow`; the terminal does not do it for them,
/// because the only correction that would is a per-dab normalisation, and
/// that is what removes per-dab stacking.
#[test]
fn a_fresh_pass_never_lightens_as_the_dial_rises() {
    let darkest: Vec<i32> = DIAL
        .iter()
        .map(|dial| {
            let mut engine = test_engine();
            let layer = engine.add_raster_layer(None);
            install_pencil(&mut engine, Some(*dial));
            stroke_centreline(&mut engine, layer, 0.7, 1);
            darkest_over_white(&engine, layer)
        })
        .collect();
    for pair in darkest.windows(2) {
        assert!(
            pair[1] <= pair[0],
            "a fresh pass must get darker or stay, never lighter: {darkest:?}"
        );
    }
    assert!(
        darkest[0] - darkest[4] > 32,
        "the dial must actually reach from one law to the other: {darkest:?}"
    );
}

/// The layer's transparency must not change the result at any dial
/// setting: the mid-dial commit lays one accumulation over another, and
/// neither step may reintroduce the dependence the deposit model removes.
///
/// Tolerance of 2 rather than the single-law test's 1: the mid-dial commit
/// runs one more straight-alpha composite, and its rounding lands in the
/// last bit.
#[test]
fn transparent_and_opaque_layers_agree_at_every_dial_setting() {
    for dial in [0.0, 0.5, 1.0] {
        let run_on = |opaque: bool| {
            let mut engine = test_engine();
            let layer = engine.add_raster_layer(None);
            if opaque {
                fill_opaque(&mut engine, layer, 255, 255, 255);
            }
            install_pencil(&mut engine, Some(dial));
            for _ in 0..3 {
                stroke_centreline(&mut engine, layer, 0.7, 1);
            }
            darkest_over_white(&engine, layer)
        };
        let (transparent, opaque) = (run_on(false), run_on(true));
        assert!(
            (transparent - opaque).abs() <= 2,
            "dial {dial}: transparent reached {transparent}, opaque {opaque}; \
             layer transparency must not change the result"
        );
    }
}

/// An eraser scrubbing its own path removes more as the dial rises, for
/// the same reason a brush deposits more: each half removes its own
/// coverage, and the stacking half compounds where the washing half caps.
#[test]
fn erase_removes_more_as_the_dial_rises() {
    let remaining: Vec<u8> = DIAL
        .iter()
        .map(|dial| {
            let mut engine = test_engine();
            let layer = engine.add_raster_layer(None);
            fill_opaque(&mut engine, layer, 0, 0, 0);
            install_pencil(&mut engine, Some(*dial));
            engine.set_brush_blend_mode(1);
            stroke_centreline(&mut engine, layer, 0.6, 4);
            *centreline_alpha(&engine, layer).iter().min().unwrap()
        })
        .collect();
    for pair in remaining.windows(2) {
        assert!(
            pair[1] <= pair[0],
            "a scrubbing eraser must remove at least as much as the setting below: {remaining:?}"
        );
    }
    assert!(
        remaining[0] > remaining[4],
        "the dial must change how an eraser compounds: {remaining:?}"
    );
}

/// The two laws, stated as arithmetic rather than as one brush's look.
///
/// At the bottom of the dial a pixel reads one dab however many landed on
/// it (`Max` of equal dabs); at the top it reads them composited over each
/// other, `1 - (1 - a)^k`. So the ratio of `ln(1 - b)` to `ln(1 - a)` is
/// the number of dabs that landed, a whole number of at least two for a
/// stroke short enough to sit inside one dab.
///
/// The dab count is measured, never assumed: the segment walk starts at
/// zero distance travelled, so a two-event stroke places three dabs, not
/// two. This test is what a per-dab normalisation would break, whatever it
/// did to any particular brush's tuning.
#[test]
fn the_top_of_the_dial_stacks_dabs_by_source_over() {
    let centre_alpha = |dial: f32| {
        let mut engine = test_engine();
        let layer = engine.add_raster_layer(None);
        // Calligraphy wires neither flow, so both halves take the value
        // this test authors and the arithmetic is about the law alone.
        install_builtin(&mut engine, "Calligraphy");
        set_buildup(&mut engine, dial);
        let term = find_node_id(&engine, "paint");
        for port in ["wash_flow", "build_flow"] {
            engine
                .brush_graph_set_input(&term, port, InputValue::Scalar(0.25))
                .expect("paint flow port");
        }
        // A hard-edged round disc wide enough that a short stroke stays
        // inside every dab it places, so the centre pixel sees them all at
        // full strength and the falloff never enters the arithmetic.
        let circle = find_node_id(&engine, "circle");
        for (port, value) in [("softness", 0.0), ("aspect", 1.0)] {
            engine
                .brush_graph_set_input(&circle, port, InputValue::Scalar(value))
                .expect("circle port");
        }
        let bs = find_node_id(&engine, "brush_settings");
        engine
            .brush_graph_set_input(&bs, "size", InputValue::Scalar(0.5))
            .expect("brush_settings size port");
        engine
            .brush_graph_set_input(&bs, "stabilize", InputValue::Scalar(0.0))
            .expect("brush_settings stabilize port");

        engine.begin_stroke(layer).unwrap();
        for i in 0..2 {
            engine.stroke_to(StrokeOp::BrushStroke {
                x: (W / 2) as f32 + i as f32 * 2.0,
                y: (H / 2) as f32,
                pressure: 1.0,
                x_tilt: 0.0,
                y_tilt: 0.0,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: i as f64 * 16.0,
                cr: 0.0,
                cg: 0.0,
                cb: 0.0,
                ca: 1.0,
            });
        }
        engine.end_stroke();
        engine.test_flush_readbacks();
        let px = engine.test_readback_layer(layer);
        let i = (((H / 2) * W + W / 2) * 4) as usize;
        px[i + 3] as f32 / 255.0
    };

    let a = centre_alpha(0.0);
    assert!(
        (a - 0.25).abs() < 0.02,
        "one dab should land at the authored flow, 0.25; read {a}"
    );

    let b = centre_alpha(1.0);
    let k = (1.0 - b).ln() / (1.0 - a).ln();
    assert!(
        (k - k.round()).abs() < 0.1 && k.round() >= 2.0,
        "the same dabs composited over each other must read as a whole \
         number of them, at least two: one dab {a}, the stroke {b}, ratio {k}"
    );
}

/// The dial the shipped Pencil authors in YAML reaches *both* reads.
///
/// Every other test here calls `brush_graph_set_input` to select the law,
/// which exercises the runtime path and silently skips the one the app
/// actually uses: the value `PortableBrush::graph_from_nodes` seeds from
/// `pencil.yaml`. The compile-time read (which picks the per-dab blend and
/// whether a second accumulation exists at all) and the commit-time read
/// (which picks the two slots' opacities) are separate lookups, so a brush
/// could plausibly get one and not the other.
///
/// Two things have to hold, and between them they close that gap. The
/// authored brush sits strictly inside the dial at both overlap sites,
/// measured against the same brush driven to each end through the runtime
/// path, which is what keeps this a statement about the YAML rather than
/// about the Pencil's current tuning. And the two sites agree with each
/// other: a compile read that took the authored value while the commit read
/// fell back to the registration default would show up here as a retrace
/// building a little while a second stroke compounded.
///
/// Measured surcharge in median alpha at [`PENCIL_SPACING`], pressure 0.5:
/// 0 / 13 / 62 within a stroke, 0 / 13 / 64 across two.
#[test]
fn the_shipped_pencils_authored_dial_reaches_both_reads() {
    /// Clear of the per-site grain by enough that the ordering is the
    /// measurement and not the noise.
    const INSIDE_MARGIN: i32 = 6;

    let ladder = |site: fn(Option<f32>, f32) -> i32| {
        [
            site(Some(WASH), PENCIL_SPACING),
            site(None, PENCIL_SPACING),
            site(Some(BUILD_UP), PENCIL_SPACING),
        ]
    };
    let within = ladder(within_stroke_surcharge);
    let across = ladder(across_stroke_surcharge);

    for (site, rungs) in [("a retrace", within), ("a second stroke", across)] {
        assert!(
            rungs[0] + INSIDE_MARGIN < rungs[1] && rungs[1] + INSIDE_MARGIN < rungs[2],
            "{site}: the Pencil as authored must build more than Wash and less \
             than Build-up: {rungs:?}"
        );
    }

    assert!(
        (within[1] - across[1]).abs() <= SITE_PARITY_TOL,
        "as authored, a retrace added {} but a second stroke added {}; the \
         compile-time and commit-time reads of `buildup` must agree",
        within[1],
        across[1]
    );
}
