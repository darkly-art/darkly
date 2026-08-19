//! Tests for the compiled `liquify` terminal.
//!
//! The load-bearing invariant — same as smudge — is the
//! per-dab feedback loop: dab 2's warp source samples scratch *after*
//! dab 1 has written to it. A single instanced draw would have both
//! dabs reading pre-stroke. The discriminator test places two dabs so
//! that dab 2's centre-fragment warp source coincides with dab 1's
//! centre, where dab 1 deposited a warped pixel from a known
//! pre-stroke region. Without the per-dab `copy_texture_to_texture`
//! barrier, dab 2 would read pre-stroke (BLACK) instead of dab 1's
//! deposit (RED).

use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, StrokeResources};
use darkly::brush::nodes::liquify::LIQUIFY_SPACING_RATIO;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};

const CANVAS: u32 = 128;

/// Dab step these tests hand-place at, in canvas pixels. Independent of
/// the brush's configured spacing: the harness places dabs at explicit
/// positions and synthesises the matching `pen.motion`, so this is the
/// tests' own geometry, not a copy of the brush's.
const TEST_DAB_STEP_PX: f32 = 4.0;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

/// Red vertical bar in `x < red_x_threshold`; opaque black elsewhere.
fn two_tone_canvas(red_x_threshold: u32) -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let idx = ((y * CANVAS + x) * 4) as usize;
            if x < red_x_threshold {
                out[idx] = 220;
                out[idx + 1] = 20;
                out[idx + 2] = 20;
            }
            out[idx + 3] = 255;
        }
    }
    out
}

/// 4 px-period vertical stripes: maximum frequency along the drag axis,
/// and exactly two tones — so any intermediate red value in the output is
/// resampling loss, not content.
fn stripe_canvas() -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let idx = ((y * CANVAS + x) * 4) as usize;
            if (x / 2) % 2 == 1 {
                out[idx] = 255;
            }
            out[idx + 3] = 255;
        }
    }
    out
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * CANVAS + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

/// One `(pos, direction_rad, distance)` per dab — the direction sets the
/// per-dab motion vector. `distance > 0.5` so the first-dab gate doesn't fire.
fn render_liquify_dabs(size_override: f32, dabs: &[([f32; 2], f32, f32)]) -> Vec<u8> {
    render_liquify_dabs_on(&two_tone_canvas(36), size_override, dabs)
}

fn render_liquify_dabs_on(
    canvas: &[u8],
    size_override: f32,
    dabs: &[([f32; 2], f32, f32)],
) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Liquify")
        .unwrap();

    let mut graph = brush.metadata.graph.clone();
    let term_id = darkly::brush::find_terminal(&graph).expect("Liquify brush has a terminal");
    graph
        .set_port_default(
            &darkly::brush::nodes::brush_settings::node_id(&graph).unwrap(),
            "size",
            size_override,
        )
        .unwrap();
    // Push strength to max so the test's warp is unambiguous.
    graph.set_port_default(&term_id, "strength", 1.0).unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) = create_test_texture(&device, &queue, CANVAS, CANVAS, canvas);
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    // Compile before allocating: the terminal decides the scratch format,
    // and liquify's is a warp field rather than colour.
    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");
    let mut stroke_buffer =
        StrokeBuffer::new(&device, CANVAS, CANVAS, &pipelines, runner.scratch_format());

    let pre_stroke = darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        darkly::coord::CanvasRect::from_xywh(0, 0, CANVAS, CANVAS),
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("liquify-test-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke);
    queue.submit([enc.finish()]);

    macro_rules! make_ctx {
        ($label:expr) => {{
            let (scratch, pre_stroke_tex, pre_stroke_bg, source_override) =
                stroke_buffer.parts_for_brush_ctx();
            BrushGpuContext {
                encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some($label),
                }),
                device: &device,
                queue: &queue,
                pipelines: &pipelines,
                selection_bind_group: pipelines.default_selection_bind_group(),
                canvas_width: CANVAS,
                canvas_height: CANVAS,
                canvas_origin: [0, 0],
                blend_mode: 0,
                view_rotation: 0.0,
                perf: BrushPerfCounters::default(),
                stroke: Some(StrokeResources {
                    scratch,
                    paint_target: darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
                        &layer_texture,
                        &layer_view,
                        wgpu::TextureFormat::Rgba8Unorm,
                        darkly::coord::CanvasRect::from_xywh(0, 0, CANVAS, CANVAS),
                    ),
                    pre_stroke_texture: pre_stroke_tex,
                    pre_stroke_bind_group: pre_stroke_bg,
                    source_override,
                }),
                preview: None,
                dab_batch: DabBatch::default(),
            }
        }};
    }

    {
        let mut ctx = make_ctx!("liquify-test-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("liquify-test-flush");
        for (i, (pos, dir, dist)) in dabs.iter().enumerate() {
            // Simulate a real stroke's per-dab motion: in a live
            // stroke the engine places dabs a spacing apart along
            // the cursor's path, so `pen.motion` per dab has that
            // magnitude along the drawing angle. `motion` is the only
            // direction signal liquify consumes.
            let motion = [TEST_DAB_STEP_PX * dir.cos(), TEST_DAB_STEP_PX * dir.sin()];
            let info = PaintInformation {
                pos: *pos,
                distance: *dist,
                motion,
                pressure: 1.0,
                ..Default::default()
            };
            runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0xC0FFEE, i as u32);
            runner.execute_cpu();
            runner.execute_gpu(&mut ctx);
        }
        runner.flush_dabs(&mut ctx);
        runner.commit(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }

    readback_texture(
        &device,
        &queue,
        &layer_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        CANVAS,
        CANVAS,
    )
}

/// **Regression test for the ghosting bug.** Liquify used to resample the
/// *image* once per dab: each dab snapshotted the scratch, sampled it at a
/// displaced UV and wrote the colour straight back. With 4 px dab spacing
/// and a 30.7 px radius a material point passed through ~15 bilinear
/// filters per swipe, and their composition is not a bilinear filter — it
/// is a low-pass cascade. Detail decayed monotonically with dab count,
/// which is what "liquify ghosts everything" meant.
///
/// Accumulating a displacement field and resampling the pre-stroke image
/// exactly once holds detail constant no matter how many dabs pass over a
/// pixel.
///
/// Two assertions, and the pairing is the point — neither alone is
/// sufficient:
///
/// * **A (sharpness)** is what fails under the bug, but is satisfied
///   perfectly by a liquify that displaces nothing (pristine stripes).
/// * **B (displacement)** proves content actually moved, and passes both
///   before and after the fix.
///
/// Only an implementation that moves content *and* keeps it sharp passes
/// both.
#[test]
fn liquify_scrubbing_preserves_high_frequency_detail() {
    // 4 back-and-forth passes between x=40 and x=90 at y=64, 4 px apart.
    let mut dabs: Vec<([f32; 2], f32, f32)> = Vec::new();
    let mut distance = 4.0_f32;
    let mut x = 40.0_f32;
    for pass in 0..4 {
        let dir = if pass % 2 == 0 {
            0.0
        } else {
            std::f32::consts::PI
        };
        let step = if pass % 2 == 0 {
            TEST_DAB_STEP_PX
        } else {
            -TEST_DAB_STEP_PX
        };
        for _ in 0..12 {
            x += step;
            distance += TEST_DAB_STEP_PX;
            dabs.push(([x, 64.0], dir, distance));
        }
    }

    let source = stripe_canvas();
    // size 0.12 → radius = 0.12 * DAB_REFERENCE_SIZE * 0.5 = 30.72 px.
    let rgba = render_liquify_dabs_on(&source, 0.12, &dabs);

    // A — sharpness. Every row crossing the stroke must still contain both
    // tones. Source scores 255; the pre-fix implementation scored 0–42.
    let mut worst = (255u8, 0u32);
    for y in 44..86 {
        let (mut lo, mut hi) = (255u8, 0u8);
        for x in 45..90 {
            let r = pixel(&rgba, x, y)[0];
            lo = lo.min(r);
            hi = hi.max(r);
        }
        let ptp = hi - lo;
        if ptp < worst.0 {
            worst = (ptp, y);
        }
    }
    assert!(
        worst.0 >= 150,
        "liquify must not low-pass the image it warps: row {} has red \
         peak-to-peak {} (need >= 150). The source is a two-tone stripe \
         pattern, so anything in between is resampling loss — this is the \
         ghosting bug, and it means liquify is resampling the picture per \
         dab instead of accumulating a displacement field.",
        worst.1,
        worst.0,
    );

    // B — displacement. A liquify that does nothing would sail through A.
    let mut moved = 0u32;
    let mut total = 0u32;
    for y in 44..86 {
        for x in 45..90 {
            let before = pixel(&source, x, y)[0] as i32;
            let after = pixel(&rgba, x, y)[0] as i32;
            total += 1;
            if (after - before).abs() >= 100 {
                moved += 1;
            }
        }
    }
    let fraction = moved as f32 / total as f32;
    assert!(
        fraction >= 0.25,
        "liquify must actually displace content: only {:.1}% of the \
         stroked region changed by >= 100 (need >= 25%). A no-op liquify \
         scores 0% here while still passing the sharpness assertion.",
        fraction * 100.0,
    );
}

/// Confidence test: a single liquify dab at (38, 64) pulling
/// rightward (direction = 0, strength = 1) lifts red into the dab
/// centre. With `|motion| = TEST_DAB_STEP_PX = 4`, displacement at
/// strength=1 is 4 px, so the centre fragment sources from (34, 64)
/// — inside the red bar at `x < 36`. (Size is irrelevant to the
/// per-dab displacement now — kept at 0.3 only so the disc actually
/// covers both the centre and the source.)
#[test]
fn single_liquify_dab_warps_red_into_center() {
    let rgba = render_liquify_dabs(0.3, &[([38.0, 64.0], 0.0, 10.0)]);
    let centre = pixel(&rgba, 38, 64);
    assert!(
        centre[0] > 150,
        "single liquify dab warping from the red bar should deposit \
         red at the centre; got {centre:?}"
    );
    assert!(
        centre[0] > centre[1] + 50,
        "warped pixel should be clearly red, not noise; got {centre:?}"
    );
}

/// **Per-dab feedback test.** Dab 2 placed so its centre-fragment
/// source lands on dab 1's centre, where dab 1 warped a red pixel
/// from the pre-stroke red bar. Dab 2's centre fragment must see
/// dab 1's RED deposit through the inter-dab scratch barrier; if
/// dab 2 reads pre-stroke at dab 1's centre it gets BLACK (the
/// pre-stroke at x = 38 is past the red bar at x < 36).
#[test]
fn liquify_dab2_reads_dab1_deposit_not_pre_stroke() {
    // `|motion| = TEST_DAB_STEP_PX = 4` → displacement at strength=1
    // is 4 px, independent of brush size.
    let rgba = render_liquify_dabs(
        0.3,
        &[
            // Dab 1 at (38, 64): centre source at (34, 64) —
            // inside the red bar.
            ([38.0, 64.0], 0.0, 10.0),
            // Dab 2 at (42, 64): centre source at (38, 64) —
            // coincides with dab 1's centre where the red deposit
            // lives.
            ([42.0, 64.0], 0.0, 20.0),
        ],
    );
    let centre_2 = pixel(&rgba, 42, 64);
    assert!(
        centre_2[0] > 120,
        "dab 2's warp source must read dab 1's red deposit through \
         the per-dab barrier — got centre {centre_2:?}. Pre-stroke at \
         (38, 64) was BLACK; if dab 2 sees this value it means the \
         inter-dab `copy_texture_to_texture` (and thus the per-dab \
         serialization) is broken."
    );
    assert!(
        centre_2[0] > centre_2[1] + 50,
        "dab 2 reading dab 1's red deposit should leave red dominant; \
         got {centre_2:?}"
    );
}

/// Regression: per-dab displacement must NOT scale with brush radius.
/// The size slider controls the warped *extent* (the disc), not its
/// *intensity*.
///
/// Both runs: one eastward dab at (38, 64) with strength=1 and
/// `|motion| = TEST_DAB_STEP_PX = 4`. The pre-stroke red bar lives
/// at `x < 36`. With the (now-fixed) formula `displacement = strength
/// × |motion| = 4 px`, a fragment at (42, 64) samples from (38, 64)
/// — background. The brush centre at (38, 64) samples from (34, 64)
/// — well inside the red bar — confirming the warp is actually
/// running (not silently zero).
///
/// Under the previous radius-coupled formula `displacement = 0.08 ×
/// radius × strength`, the large brush (size=1.0, radius=256) gave
/// displacement = 20.48 px, so (42, 64) would have sampled from
/// (~21.5, 64) — well inside the red bar — and read RED. The test
/// fails loudly if that coupling comes back.
#[test]
fn warp_magnitude_is_size_invariant() {
    // Small brush (radius=76.8) — positive control: warp ran at all.
    // Centre (38, 64) samples from (34, 64) — inside the red bar.
    let small = render_liquify_dabs(0.3, &[([38.0, 64.0], 0.0, 10.0)]);
    let small_at_centre = pixel(&small, 38, 64);
    assert!(
        small_at_centre[0] > 150,
        "small brush: centre should be red (warp ran), got \
         {small_at_centre:?}"
    );
    let small_at_42 = pixel(&small, 42, 64);
    assert!(
        small_at_42[0] < 60,
        "small brush: (42, 64) should sample from background, got \
         {small_at_42:?}"
    );

    // Large brush (radius=256) — the discriminator. Same |motion|
    // and strength, so the same displacement (4 px). (42, 64) must
    // still sample from background; under any radius-coupled formula
    // displacement at this size would be much larger and (42, 64)
    // would land inside the red bar.
    let large = render_liquify_dabs(1.0, &[([38.0, 64.0], 0.0, 10.0)]);
    let large_at_centre = pixel(&large, 38, 64);
    assert!(
        large_at_centre[0] > 150,
        "large brush: centre should also be red (size doesn't change \
         displacement), got {large_at_centre:?}"
    );
    let large_at_42 = pixel(&large, 42, 64);
    assert!(
        large_at_42[0] < 60,
        "large brush: (42, 64) must still sample from background — \
         if this is red the radius-coupled formula has come back and \
         the strength slider once again grows with brush size. Got \
         {large_at_42:?}"
    );
}

// ============================================================================
// End-to-end: the whole engine, including the checkpoint ring
// ============================================================================

/// **Guards the format-aware checkpoint ring.** Liquify's scratch holds a
/// float displacement field, and `CheckpointRing::save` snapshots the
/// scratch with `copy_texture_to_texture`, which rejects a format
/// mismatch. Before `CheckpointSlot` took its format from the source
/// texture, any liquify stroke long enough to check-point died on a wgpu
/// validation error.
///
/// It also closes the loop on the ghosting bug at the level the user
/// actually meets it: a real stroke through `DarklyEngine`, with
/// stabilization, dab scheduling, checkpointing and mid-stroke commits
/// all live — not the hand-driven dab harness the tests above use.
#[test]
fn liquify_stroke_through_engine_preserves_detail() {
    use darkly::engine::types::StrokeOp;
    use darkly::engine::DarklyEngine;
    use darkly::gpu::context::GpuContext;

    const SIZE: u32 = 128;
    let (device, queue) = darkly::gpu::test_utils::test_device();
    let mut engine = DarklyEngine::new(GpuContext::new_headless(device, queue), SIZE, SIZE);
    // Paste the stripes in as a layer, so the input is exactly two tones
    // rather than something painted (and therefore anti-aliased).
    let layer_id = engine.paste_image(SIZE, SIZE, &stripe_canvas(), 0, 0, None);

    let liquify_yaml = darkly::brush::builtin_brushes::BUILTIN_BRUSHES_YAML
        .iter()
        .find(|(name, _)| *name == "liquify.yaml")
        .expect("liquify brush is shipped")
        .1;
    engine
        .set_brush_graph_yaml(liquify_yaml)
        .expect("liquify brush loads");

    // A long drag: enough events to cross several checkpoint intervals.
    engine.begin_stroke(layer_id);
    for step in 0..=60 {
        let t = step as f32 / 60.0;
        engine.stroke_to(StrokeOp::BrushStroke {
            x: 30.0 + t * 70.0,
            y: 64.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: step as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer_id);
    let source = stripe_canvas();

    // The stroke must have moved something...
    let moved = (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let a = pixel(&pixels, x, y)[0] as i32;
            let b = pixel(&source, x, y)[0] as i32;
            (a - b).abs() >= 100
        })
        .count();
    assert!(
        moved > 200,
        "liquify stroke should have displaced content; only {moved} pixels changed",
    );

    // ...without dissolving the stripes into mush anywhere it touched.
    for y in 40..90 {
        let (mut lo, mut hi) = (255u8, 0u8);
        for x in 35..100 {
            let r = pixel(&pixels, x, y)[0];
            lo = lo.min(r);
            hi = hi.max(r);
        }
        assert!(
            hi - lo >= 150,
            "row {y}: red peak-to-peak {} after a full engine-driven \
             liquify stroke (need >= 150) — detail was destroyed",
            hi - lo,
        );
    }
}

/// The brush's configured spacing and [`LIQUIFY_SPACING_RATIO`] are the
/// same decision written in two files — the YAML the engine actually
/// reads, and the Rust constant whose doc comment carries the reasoning
/// (why 0.05, and what banding measurement bounds it). Pin them together
/// so neither can drift silently.
///
/// Spacing is not cosmetic here: pinning it flat in pixels, as this brush
/// used to, makes per-travel cost `O(radius²)` because the dab count stops
/// falling as the disc grows.
#[test]
fn shipped_liquify_spacing_matches_the_declared_ratio() {
    let graph = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Liquify")
        .expect("Liquify is shipped")
        .metadata
        .graph;
    let spacing = darkly::brush::nodes::brush_settings::spacing_config(&graph);

    assert!(
        (spacing.ratio - LIQUIFY_SPACING_RATIO).abs() < 1e-6,
        "brushes/liquify.yaml sets spacing {} but LIQUIFY_SPACING_RATIO is {}",
        spacing.ratio,
        LIQUIFY_SPACING_RATIO,
    );
    assert!(
        spacing.ratio > 0.0,
        "liquify spacing must stay proportional to dab size; a zero ratio \
         falls back to the pixel floor and restores O(radius²) cost",
    );

    // A large brush must actually get large steps — the whole point.
    let big_diameter = 1000.0;
    assert!(
        spacing.distance(big_diameter) >= 40.0,
        "a {big_diameter}px-diameter liquify brush should step >= 40px, got {}",
        spacing.distance(big_diameter),
    );
}
