//! Tests for the compiled `liquify_compiled` terminal.
//!
//! The load-bearing invariant — same as smudge_compiled — is the
//! per-dab feedback loop: dab 2's warp source samples scratch *after*
//! dab 1 has written to it. A single instanced draw would have both
//! dabs reading pre-stroke. The discriminator test places two dabs so
//! that dab 2's centre-fragment warp source coincides with dab 1's
//! centre, where dab 1 deposited a warped pixel from a known
//! pre-stroke region. Without the per-dab `copy_texture_to_texture`
//! barrier, dab 2 would read pre-stroke (BLACK) instead of dab 1's
//! deposit (RED).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::dab_pool::DabTexturePool;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
use darkly::brush::nodes::liquify_compiled::LIQUIFY_SPACING_PX;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::brush::wire::TextureHandle;
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

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * CANVAS + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

/// One `(pos, direction_rad, distance)` per dab. `distance > 0.5` so
/// the per-dab first-dab gate doesn't fire.
fn render_liquify_dabs(size_override: f32, dabs: &[([f32; 2], f32, f32)]) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Liquify")
        .unwrap();

    let mut graph = brush.metadata.graph.clone();
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "liquify_compiled")
        .map(|(id, _)| *id)
        .expect("Liquify brush must terminate in liquify_compiled");
    graph
        .set_port_default(term_id, "size", size_override)
        .unwrap();
    // Push strength to max so the test's warp is unambiguous.
    graph.set_port_default(term_id, "strength", 1.0).unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, CANVAS, CANVAS, &two_tone_canvas(36));
    let mut dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout());
    let mut stroke_buffer = StrokeBuffer::new(
        &device,
        CANVAS,
        CANVAS,
        dab_pool.bind_group_layout(),
        &pipelines,
    );

    let pre_stroke = darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        CANVAS,
        CANVAS,
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("liquify_compiled-test-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke);
    queue.submit([enc.finish()]);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");
    let resources: HashMap<String, TextureHandle> = HashMap::new();

    macro_rules! make_ctx {
        ($label:expr) => {{
            let (scratch, pre_stroke_tex, pre_stroke_bg) = stroke_buffer.parts_for_brush_ctx();
            BrushGpuContext {
                encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some($label),
                }),
                device: &device,
                queue: &queue,
                dab_pool: &mut dab_pool,
                pipelines: &pipelines,
                scratch: Some(scratch),
                canvas_width: CANVAS,
                canvas_height: CANVAS,
                paint_target: Some(
                    darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
                        &layer_texture,
                        &layer_view,
                        wgpu::TextureFormat::Rgba8Unorm,
                        CANVAS,
                        CANVAS,
                    ),
                ),
                selection_bind_group: pipelines.default_selection_bind_group(),
                preview_target_view: None,
                resource_handles: &resources,
                blend_mode: 0,
                preview_mask_view: None,
                preview_mask_size: (0, 0),
                brush_preview_info: None,
                pre_stroke_texture: Some(pre_stroke_tex),
                pre_stroke_bind_group: Some(pre_stroke_bg),
                dab_write_canvas_bbox: None,
                perf: BrushPerfCounters::default(),
                pending_dab_bytes: Vec::new(),
                pending_dab_count: 0,
                pending_dabs_bbox: None,
                pending_dab_meta_bytes: Vec::new(),
                compiled_brush: None,
                slot_outputs_owned: None,
            }
        }};
    }

    {
        let mut ctx = make_ctx!("liquify_compiled-test-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("liquify_compiled-test-flush");
        for (i, (pos, dir, dist)) in dabs.iter().enumerate() {
            // Simulate a real stroke's per-dab motion: in a live
            // stroke the engine places dabs `LIQUIFY_SPACING_PX`
            // apart along the cursor's path, so `pen.motion` per
            // dab has magnitude ≈ `LIQUIFY_SPACING_PX` along the
            // drawing angle.
            let motion = [
                LIQUIFY_SPACING_PX * dir.cos(),
                LIQUIFY_SPACING_PX * dir.sin(),
            ];
            let info = PaintInformation {
                pos: *pos,
                drawing_angle: *dir,
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

/// Confidence test: a single liquify dab at (38, 64) pulling
/// rightward (direction = 0, strength = 1) lifts red into the dab
/// centre. With `|motion| = LIQUIFY_SPACING_PX = 4`, displacement at
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
    // `|motion| = LIQUIFY_SPACING_PX = 4` → displacement at strength=1
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
/// `|motion| = LIQUIFY_SPACING_PX = 4`. The pre-stroke red bar lives
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
