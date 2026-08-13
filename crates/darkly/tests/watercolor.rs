//! Smoke tests for the watercolor brushes after migration to the
//! compiled `watercolor` terminal. Each test loads the actual
//! builtin graph, renders a couple of dabs over a non-empty
//! pre_stroke, and checks the watercolor blend deposits something
//! reasonable. The pickup atlas pass + per-brush compiled composite
//! pass are exercised end-to-end.

use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, StrokeResources};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};

const CANVAS: u32 = 128;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

/// Light blue initial canvas (alpha = 1.0) so pickup has something to
/// mix into the load. Watercolor's `mix(canvas_rgb, fg_color.rgb,
/// deposit)` blends pre_stroke pixels with the brush color.
fn light_blue_canvas() -> Vec<u8> {
    solid_canvas([100, 150, 230, 255])
}

fn solid_canvas(rgba: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&rgba);
    }
    out
}

/// One `flush_dabs` worth of dabs, plus whether the stroke restarts in
/// front of it.
///
/// Flush boundaries are semantically load-bearing for watercolor: the
/// pickup atlas is rebuilt once per flush, so a mark's buildup depends on
/// how the dabs are grouped. Tests must therefore be able to say "these
/// dabs in one flush" versus "one dab per flush" — hence groups rather
/// than a flat dab list.
struct FlushGroup<'a> {
    dabs: &'a [(f32, f32)],
    /// Re-enter `begin_stroke` before this group — the stabilizer-rewind
    /// path. `begin_stroke` always runs before the first group; this only
    /// affects later ones.
    restart: bool,
    /// Brush colour for this group, overriding the render call's default.
    color: Option<[f32; 4]>,
}

fn group(dabs: &[(f32, f32)]) -> FlushGroup<'_> {
    FlushGroup {
        dabs,
        restart: false,
        color: None,
    }
}

fn restart_group(dabs: &[(f32, f32)]) -> FlushGroup<'_> {
    FlushGroup {
        dabs,
        restart: true,
        color: None,
    }
}

impl<'a> FlushGroup<'a> {
    fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = Some(color);
        self
    }
}

/// `n` groups of one dab each at the same spot — the shape that exposes
/// cross-flush pigment buildup.
fn repeat_at(pos: (f32, f32), n: usize) -> Vec<[(f32, f32); 1]> {
    vec![[pos]; n]
}

fn groups_of<'a>(runs: &'a [[(f32, f32); 1]]) -> Vec<FlushGroup<'a>> {
    runs.iter().map(|r| group(r.as_slice())).collect()
}

fn render_dabs(
    brush_name: &str,
    size_override: f32,
    color: [f32; 4],
    dabs: &[(f32, f32)],
) -> Vec<u8> {
    render_dabs_on(brush_name, size_override, color, dabs, &light_blue_canvas())
}

fn render_dabs_on(
    brush_name: &str,
    size_override: f32,
    color: [f32; 4],
    dabs: &[(f32, f32)],
    canvas: &[u8],
) -> Vec<u8> {
    render_flush_groups(brush_name, size_override, color, &[group(dabs)], canvas)
}

/// The single rendering primitive every test in this file goes through.
///
/// Each group gets its own `BrushGpuContext` and its own `queue.submit()`,
/// mirroring production where each render phase has its own encoder and
/// `submit_final` (`engine/painting.rs`). **The submit between groups is
/// load-bearing, not incidental:** `flush_dabs` writes the dab buffer and
/// the uniform rings through `queue.write_buffer`, which is on the queue
/// timeline rather than the encoder timeline. Two `flush_dabs` calls
/// recorded into one encoder would both read the *second* batch, and a
/// multi-flush test would silently collapse into a single flush while
/// still appearing to pass.
///
/// `commit` runs once, in the final group's context.
fn render_flush_groups(
    brush_name: &str,
    size_override: f32,
    color: [f32; 4],
    groups: &[FlushGroup<'_>],
    canvas: &[u8],
) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == brush_name)
        .unwrap_or_else(|| panic!("builtin brush `{brush_name}` not registered"));

    let mut graph = brush.metadata.graph.clone();
    let _term_id = darkly::brush::find_terminal(&graph)
        .unwrap_or_else(|err| panic!("brush `{brush_name}`: {err}"));
    graph
        .set_port_default(
            &darkly::brush::nodes::brush_settings::node_id(&graph).unwrap(),
            "size",
            size_override,
        )
        .unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) = create_test_texture(&device, &queue, CANVAS, CANVAS, canvas);
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut stroke_buffer = StrokeBuffer::new(&device, CANVAS, CANVAS, &pipelines);

    let pre_stroke = darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        darkly::coord::CanvasRect::from_xywh(0, 0, CANVAS, CANVAS),
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("watercolor-compiled-test-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke);
    queue.submit([enc.finish()]);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");
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

    let mut dab_index = 0u32;
    for (gi, g) in groups.iter().enumerate() {
        let mut ctx = make_ctx!("watercolor-compiled-test-flush");
        if gi == 0 || g.restart {
            runner.begin_stroke(&mut ctx);
        }
        for (x, y) in g.dabs {
            let info = PaintInformation {
                pos: [*x, *y],
                pressure: 1.0,
                ..Default::default()
            };
            runner.seed_sensors(&info, g.color.unwrap_or(color), 0xC0FFEE, dab_index);
            runner.execute_cpu();
            runner.execute_gpu(&mut ctx);
            dab_index += 1;
        }
        runner.flush_dabs(&mut ctx);
        if gi + 1 == groups.len() {
            runner.commit(&mut ctx);
        }
        // Per-group submit — see the doc comment; without this the next
        // group's queue writes would land before this group's passes run.
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

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * CANVAS + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

#[test]
fn smooth_watercolor_deposits_blend_of_brush_and_pickup() {
    // Brush color is red; canvas is light blue. Watercolor's deposit
    // (default 0.5) gives a load that mixes both — the centre pixel
    // should have nonzero red AND retain some blue from the pickup.
    let rgba = render_dabs(
        "Smooth Watercolor",
        0.2,
        [1.0, 0.0, 0.0, 1.0],
        &[(64.0, 64.0)],
    );
    let center = pixel(&rgba, 64, 64);
    // Some red got deposited (would be 100 with no brush touch).
    assert!(
        center[0] > 130,
        "Smooth Watercolor centre should add red over the light-blue \
         pickup, got {center:?} (canvas r=100)"
    );
    // Some blue remains from the pickup mix (would be 0 if deposit=1.0
    // and pickup were ignored).
    assert!(
        center[2] > 50,
        "Smooth Watercolor centre should retain blue from the pickup \
         mix, got {center:?}"
    );

    // Far corner — outside the dab footprint, must be unchanged.
    let corner = pixel(&rgba, 10, 10);
    assert_eq!(
        corner,
        [100, 150, 230, 255],
        "outside the dab should be unchanged (commit reuses pre_stroke), got {corner:?}"
    );
}

#[test]
fn rough_watercolor_renders_multiple_dabs_in_one_flush() {
    // Two perlin dabs at different positions in one flush. Both must
    // land — verifies per-instance atlas-cell indexing through the
    // compiled composite shader.
    let rgba = render_dabs(
        "Rough Watercolor",
        0.2,
        [1.0, 0.5, 0.0, 1.0],
        &[(40.0, 64.0), (88.0, 64.0)],
    );
    // Count pixels where the red channel exceeds the canvas's red
    // (= 100). Both dabs deposit orange over light blue, so post-
    // commit those pixels should have measurably more red.
    let touched = rgba.chunks_exact(4).filter(|p| p[0] > 130).count();
    assert!(
        touched > 100,
        "Rough Watercolor: expected >100 pixels touched by two dabs, got {touched}"
    );

    // Both dab centres should show red lift. Perlin shape may not
    // cover the exact centre pixel, so check a small neighborhood
    // around each centre.
    fn lift_in_3x3(rgba: &[u8], cx: u32, cy: u32) -> u8 {
        let mut max_red = 0u8;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let p = pixel(rgba, (cx as i32 + dx) as u32, (cy as i32 + dy) as u32);
                if p[0] > max_red {
                    max_red = p[0];
                }
            }
        }
        max_red
    }
    assert!(
        lift_in_3x3(&rgba, 40, 64) > 130,
        "left dab centre neighborhood should have red lift"
    );
    assert!(
        lift_in_3x3(&rgba, 88, 64) > 130,
        "right dab centre neighborhood should have red lift"
    );
}

/// Regression for the stabilizer rewind artifact: when the stroke engine
/// re-enters `begin_stroke` mid-stroke (the path taken on every divergence
/// boundary — see `engine/painting.rs::brush_stroke_to` rewind branch), the
/// watercolor scratch must be cleared. The checkpoint ring restores pixels
/// inside its bbox; without a `begin_stroke` clear, pigment from the now-
/// defunct dabs persists outside the bbox and bleeds onto the layer at
/// commit time — visible as artifacts along the tops of curves.
///
/// This test reproduces that path without spinning up the full engine:
/// render a dab at (40, 64), re-enter `begin_stroke`, render a different
/// dab at (88, 64), commit. The (40, 64) position must remain unchanged
/// from pre_stroke — its pigment must have been wiped by the second
/// `begin_stroke`.
#[test]
fn begin_stroke_clears_scratch_so_rewind_drops_defunct_pigment() {
    // Small dab — at `size = 0.05` the dab radius is ~13 px (size *
    // DAB_REFERENCE_SIZE / 2 ≈ 12.8), so the two dab positions (40, 64)
    // and (88, 64) are well isolated and don't overlap.
    //
    // Two flush groups, each preceded by `begin_stroke`: the first lays
    // the defunct dab, the second is the rewind that must wipe it.
    let rgba = render_flush_groups(
        "Smooth Watercolor",
        0.05,
        [1.0, 0.0, 0.0, 1.0],
        &[
            restart_group(&[(40.0, 64.0)]),
            restart_group(&[(88.0, 64.0)]),
        ],
        &light_blue_canvas(),
    );

    // The defunct dab at (40, 64) must be wiped. Allow ±1 LSB for rounding.
    let defunct = pixel(&rgba, 40, 64);
    let expected = [100u8, 150, 230, 255];
    for (i, (got, want)) in defunct.iter().zip(expected.iter()).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "defunct dab pixel (40, 64) channel {i}: expected {want}, got {got} (full pixel {defunct:?}) — \
             the second begin_stroke must clear stale scratch pigment so the rewind path drops the defunct stroke",
        );
    }

    // Sanity: the surviving dab at (88, 64) must still deposit red — the
    // clear must not have wiped the dab we just rendered.
    let surviving = pixel(&rgba, 88, 64);
    assert!(
        surviving[0] > 130,
        "surviving dab at (88, 64) should still show red lift, got {surviving:?}"
    );
}

/// Regression: pigment must keep building where the brush passes more than
/// once.
///
/// The watercolor pickup atlas samples the canvas under each dab to decide
/// what colour to deposit. That sample used to come from the pre-stroke
/// snapshot — a texture frozen when the stroke began — so the deposited
/// load was identical on the first pass and the twentieth. The mark
/// converged after two or three passes and then stopped changing, well
/// short of the brush colour, no matter how long the brush dwelled.
///
/// Painting one dab per flush at a fixed spot, three flushes versus eight:
/// with the pickup frozen, both land on the same colour (measured
/// `(177, 75, 116)` vs `(177, 74, 114)` — identical red, and blue moving
/// 2 LSB the *wrong* way). Reading the live canvas instead, the eight-pass
/// mark is visibly further along toward red.
#[test]
fn watercolor_pigment_builds_up_across_flushes() {
    let canvas = light_blue_canvas();
    let runs_3 = repeat_at((64.0, 64.0), 3);
    let runs_8 = repeat_at((64.0, 64.0), 8);

    let after_3 = pixel(
        &render_flush_groups(
            "Smooth Watercolor",
            0.2,
            [1.0, 0.0, 0.0, 1.0],
            &groups_of(&runs_3),
            &canvas,
        ),
        64,
        64,
    );
    let after_8 = pixel(
        &render_flush_groups(
            "Smooth Watercolor",
            0.2,
            [1.0, 0.0, 0.0, 1.0],
            &groups_of(&runs_8),
            &canvas,
        ),
        64,
        64,
    );

    // Margin 8 clears the 1–2 LSB of rounding headroom by ~4×.
    assert!(
        after_8[0] > after_3[0] + 8,
        "red must keep building past three passes: 3 flushes {after_3:?}, 8 flushes {after_8:?}",
    );
    assert!(
        after_8[2] + 8 < after_3[2],
        "blue must keep receding past three passes: 3 flushes {after_3:?}, 8 flushes {after_8:?}",
    );
}

/// The pickup is a *neighbourhood* average, not a point sample — that is
/// the whole reason the atlas pass exists. A dab laid next to wet paint
/// must pull colour from it laterally.
///
/// Without this, `watercolor_pigment_builds_up_across_flushes` would pass
/// for any per-flush-varying input at all; this pins the mechanism.
#[test]
fn watercolor_pickup_bleeds_neighbouring_wet_paint() {
    let canvas = light_blue_canvas();
    // At size 0.2 the dab radius is 51.2 px (0.2 × DAB_REFERENCE_SIZE × 0.5)
    // and `pickup_size` defaults to 1.0, so the target dab's pickup window
    // is also ±51.2 px about its centre.
    //
    // The probe pixel is the crux. The atlas holds ONE pickup value per dab,
    // sampled around that dab's centre and then applied across its whole
    // footprint. So probe at a pixel the *target* covers but the neighbour
    // does not: the only way the neighbour's colour can reach it is through
    // the target's pickup.
    //
    //   target   (64, 64) covers x ∈ [12.8, 115.2]
    //   neighbour(94, 64) covers x ∈ [42.8, 145.2]
    //   probe    (30, 64) — inside the target, 12.8 px clear of the neighbour,
    //                       and the neighbour's paint sits inside the
    //                       target's [12.8, 115.2] pickup window.
    let neighbour = (94.0, 64.0);
    let target = (64.0, 64.0);
    const PROBE: (u32, u32) = (30, 64);

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    // A red dab in the first flush, then white over the overlap.
    let with_neighbour = render_flush_groups(
        "Smooth Watercolor",
        0.2,
        WHITE,
        &[
            group(&[neighbour]).with_color(RED),
            group(&[target]).with_color(WHITE),
        ],
        &canvas,
    );
    // The same white dab, but the first flush lays white too — identical
    // flush structure and coverage, only the neighbour's colour differs.
    let alone = render_flush_groups(
        "Smooth Watercolor",
        0.2,
        WHITE,
        &[
            group(&[neighbour]).with_color(WHITE),
            group(&[target]).with_color(WHITE),
        ],
        &canvas,
    );

    let bled = pixel(&with_neighbour, PROBE.0, PROBE.1);
    let clean = pixel(&alone, PROBE.0, PROBE.1);
    // Both runs paint white here with identical geometry and identical
    // flush structure; only the neighbour's colour differs. A white brush
    // over a red-tinted pickup lands pinker — less green and less blue —
    // than the same brush over a white pickup.
    assert!(
        bled[1] + 4 < clean[1] && bled[2] + 4 < clean[2],
        "the target dab's pickup should carry its red neighbour's wet paint to {PROBE:?}, \
         which the neighbour itself never covers: with red neighbour {bled:?}, \
         with white neighbour {clean:?}",
    );
}

/// Buildup must also work from nothing. On an empty layer the pickup
/// alpha is zero, so the pickup branch stays disabled and watercolor
/// degenerates to plain paint on the first flush; from the second flush
/// on, the wet paint it just laid down is what it picks up. Guards
/// against an unpremultiply-by-zero drift toward black and against alpha
/// running away.
#[test]
fn watercolor_builds_up_on_transparent_canvas() {
    let transparent = solid_canvas([0, 0, 0, 0]);
    let runs_1 = repeat_at((64.0, 64.0), 1);
    let runs_6 = repeat_at((64.0, 64.0), 6);

    let after_1 = pixel(
        &render_flush_groups(
            "Smooth Watercolor",
            0.2,
            [1.0, 0.0, 0.0, 1.0],
            &groups_of(&runs_1),
            &transparent,
        ),
        64,
        64,
    );
    let after_6 = pixel(
        &render_flush_groups(
            "Smooth Watercolor",
            0.2,
            [1.0, 0.0, 0.0, 1.0],
            &groups_of(&runs_6),
            &transparent,
        ),
        64,
        64,
    );

    assert!(
        after_6[3] > after_1[3],
        "coverage must strengthen across flushes on an empty layer: 1 flush {after_1:?}, 6 flushes {after_6:?}",
    );
    assert!(
        after_6[0] >= after_1[0],
        "red must not drift backwards (unpremultiply-by-zero would pull toward black): \
         1 flush {after_1:?}, 6 flushes {after_6:?}",
    );
    assert!(
        after_6[1] < 40 && after_6[2] < 40,
        "a pure red brush must stay red, not grey out: 6 flushes {after_6:?}",
    );
}

/// A mark must depend only on where the dabs are, never on how they were
/// batched into `flush_dabs` calls.
///
/// Flush boundaries fall on pen events, so a grouping-dependent mark bands
/// at whatever spatial period the pen happened to report at — light patches
/// close together in a slow stroke, far apart in a fast one, and neither
/// under the artist's control. This is the regression test for that
/// banding: a straight run of dabs rendered as 1, 6, 3 and 1 flushes must
/// produce the same flat profile every time.
#[test]
fn watercolor_mark_is_invariant_to_flush_grouping() {
    const RADIUS: f32 = 7.68; // size 0.03 × 256
    let spacing = 0.1 * 2.0 * RADIUS;
    let black = solid_canvas([0, 0, 0, 255]);

    let dabs: Vec<(f32, f32)> = (0..59).map(|i| (20.0 + i as f32 * spacing, 64.0)).collect();

    let mut means: Vec<f32> = Vec::new();
    for k in [1usize, 10, 20, 59] {
        let chunks: Vec<&[(f32, f32)]> = dabs.chunks(k).collect();
        let groups: Vec<FlushGroup<'_>> = chunks.iter().map(|c| group(c)).collect();
        let rgba = render_flush_groups(
            "Smooth Watercolor",
            0.03,
            [1.0, 1.0, 1.0, 1.0],
            &groups,
            &black,
        );
        // Sample the stroke interior only — the caps taper by construction.
        let profile: Vec<u8> = (25..105).map(|x| pixel(&rgba, x, 64)[0]).collect();
        let lo = *profile.iter().min().unwrap();
        let hi = *profile.iter().max().unwrap();
        assert!(
            hi - lo <= 4,
            "mark must be flat along a straight run, but with {k} dab(s) per flush it \
             varies {lo}..{hi} — that spread is per-flush banding: {profile:?}",
        );
        means.push(profile.iter().map(|&v| v as f32).sum::<f32>() / profile.len() as f32);
    }

    let lo = means.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = means.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        hi - lo <= 4.0,
        "the same dabs must mark the same regardless of flush grouping, but the mean \
         differs by {:.1} across groupings (1/10/20/59 dabs per flush): {means:?}",
        hi - lo,
    );
}
