//! Tests for the compiled `smudge_compiled` terminal.
//!
//! The load-bearing invariant is that each dab in a phase sees the
//! *prior* dab's writeback through the scratch read mirror — a single
//! instanced draw can't express that, which is why the terminal runs
//! a per-dab fragment pass with a `copy_texture_to_texture` between
//! dabs (the implicit barrier). The discriminator test places two
//! overlapping dabs where dab 2's smear sample lands *inside* dab 1's
//! write footprint, and asserts the post-flush pixel under dab 2
//! reflects dab 1's deposit — not the unmodified pre-stroke. If the
//! per-dab barrier ever regresses (or someone collapses the flush
//! into a single instanced draw), dab 2 would read pre-stroke and
//! the test would fail.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::dab_pool::DabTexturePool;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
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

/// Pre-stroke canvas with a vertical red bar in `x < red_x_threshold`,
/// everything else opaque black. Gives the smudge a directional source
/// region: dab 1 (placed to the right of the bar) reads red across its
/// motion-offset; dab 2 (placed further right) reads dab 1's
/// recently-written red-tinted black.
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

/// One `(pos, motion)` tuple per dab. Dabs run inside a single phase —
/// `execute_gpu` queues each, then one `flush_dabs` drives the
/// per-dab render-pass loop.
fn render_smudge_dabs(size_override: f32, dabs: &[([f32; 2], [f32; 2])]) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Smudge")
        .unwrap();

    let mut graph = brush.metadata.graph.clone();
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "smudge_compiled")
        .map(|(id, _)| *id)
        .expect("Smudge brush must terminate in smudge_compiled");
    graph
        .set_port_default(term_id, "size", size_override)
        .unwrap();
    // Push rate up so the test's red-transfer is unambiguous.
    graph.set_port_default(term_id, "rate", 0.85).unwrap();

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
        label: Some("smudge_compiled-test-pre-stroke"),
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
        let mut ctx = make_ctx!("smudge_compiled-test-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("smudge_compiled-test-flush");
        for (i, (pos, motion)) in dabs.iter().enumerate() {
            let info = PaintInformation {
                pos: *pos,
                motion: *motion,
                pressure: 1.0,
                // distance > 0 so the per-pixel falloff doesn't gate
                // on the first dab (not used by smudge, but the runner
                // seeds it regardless).
                distance: 10.0,
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

/// Confidence test: a single dab on a canvas that has red to its
/// left writes a smeared red tint at the dab centre. Establishes
/// the baseline so the cross-dab feedback test can compare against
/// it.
#[test]
fn single_smudge_dab_pulls_red_via_motion() {
    // Dab at (60, 64) with motion (+30, 0) — the pen "moved right by
    // 30 px", so the smear sample is at (60-30, 64) = (30, 64), inside
    // the red bar.
    let rgba = render_smudge_dabs(0.1, &[([60.0, 64.0], [30.0, 0.0])]);
    let centre = pixel(&rgba, 60, 64);
    assert!(
        centre[0] > 80,
        "single smudge dab pulling from a red region should lift red at \
         the centre; got {centre:?}"
    );
    assert!(
        centre[0] > centre[1] + 30,
        "smear pulled from red should leave red > green; got {centre:?}"
    );
}

/// **Per-dab feedback test.** Two overlapping dabs where dab 2's
/// smear sample lands inside dab 1's write footprint. Dab 1 deposits
/// red at (60, 64) by smearing from the red bar; dab 2 at (90, 64)
/// smears from (60, 64), where dab 1 just wrote.
///
/// Working barrier: dab 2 reads dab 1's red-tinted black, depositing
/// red at (90, 64).
///
/// Broken barrier (e.g. all dabs collapsed to one instanced draw):
/// dab 2 reads pre-stroke at (60, 64), which is unmodified BLACK,
/// and (90, 64) stays nearly black.
#[test]
fn smudge_dab2_reads_dab1_deposit_not_pre_stroke() {
    let rgba = render_smudge_dabs(
        0.1,
        &[([60.0, 64.0], [30.0, 0.0]), ([90.0, 64.0], [30.0, 0.0])],
    );
    let centre_2 = pixel(&rgba, 90, 64);
    assert!(
        centre_2[0] > 50,
        "dab 2 must read dab 1's red-tinted writeback through the per-dab \
         barrier — got centre {centre_2:?}. Pre-stroke at (60, 64) was \
         BLACK; if dab 2 sees this value it means the inter-dab \
         `copy_texture_to_texture` (and thus the per-dab serialization) \
         is broken."
    );
    // Sanity: dab 2's centre still has some smear (red > black floor),
    // and the red channel exceeds the green channel by a margin
    // consistent with a tinted output rather than a noisy fluke.
    assert!(
        centre_2[0] > centre_2[1] + 30,
        "dab 2 smear from dab 1's deposit should leave red dominant; \
         got {centre_2:?}"
    );
}
