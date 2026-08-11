//! Tests for the compiled `blur` terminal.
//!
//! Blur reads a golden-angle disc of the scratch read mirror around each
//! dab, averages the taps, and writes the softened pixel back. Two
//! behaviors are load-bearing:
//!
//! 1. **Neighborhood averaging** — a dab straddling a hard black/white
//!    edge produces an intermediate gray at the edge, while pixels
//!    outside the dab footprint stay untouched.
//! 2. **Dwell-compounding** — because each dab reads the *cumulative*
//!    scratch through the per-dab read-mirror barrier (the same barrier
//!    smudge/liquify rely on), a second overlapping pass softens further
//!    than one. If the flush were ever collapsed into a single instanced
//!    draw, the second dab would re-read the pre-stroke and the
//!    compounding would vanish.

use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, StrokeResources};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};

const CANVAS: u32 = 128;

/// Column at which the pre-stroke flips from white (left) to black
/// (right). Pixels with `x < EDGE_X` are white; the rest are black.
const EDGE_X: u32 = 64;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

/// Pre-stroke canvas: a hard vertical edge — opaque white in `x < EDGE_X`,
/// opaque black elsewhere. Gives the blur a sharp boundary to soften.
fn hard_edge_canvas() -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let idx = ((y * CANVAS + x) * 4) as usize;
            let v = if x < EDGE_X { 255 } else { 0 };
            out[idx] = v;
            out[idx + 1] = v;
            out[idx + 2] = v;
            out[idx + 3] = 255;
        }
    }
    out
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * CANVAS + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

/// Render `dabs` blur touches (one position each) over the hard-edge
/// canvas at the given size / strength / opacity, returning the
/// committed layer pixels. All dabs run in one phase: `execute_gpu`
/// queues each, then one `flush_dabs` drives the per-dab render-pass loop.
fn render_blur_dabs(size: f32, strength: f32, opacity: f32, dabs: &[[f32; 2]]) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Blur")
        .unwrap();

    let mut graph = brush.metadata.graph.clone();
    let term_id = darkly::brush::find_terminal(&graph).expect("Blur brush has a terminal");
    graph
        .set_port_default(
            &darkly::brush::nodes::brush_settings::node_id(&graph).unwrap(),
            "size",
            size,
        )
        .unwrap();
    graph
        .set_port_default(&term_id, "strength", strength)
        .unwrap();
    graph
        .set_port_default(&term_id, "opacity", opacity)
        .unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, CANVAS, CANVAS, &hard_edge_canvas());
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut stroke_buffer = StrokeBuffer::new(
        &device,
        CANVAS,
        CANVAS,
        &pipelines,
        darkly::brush::node::COLOR_SCRATCH_FORMAT,
    );

    let pre_stroke = darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        darkly::coord::CanvasRect::from_xywh(0, 0, CANVAS, CANVAS),
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("blur-test-pre-stroke"),
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

    {
        let mut ctx = make_ctx!("blur-test-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("blur-test-flush");
        for (i, pos) in dabs.iter().enumerate() {
            let info = PaintInformation {
                pos: *pos,
                pressure: 1.0,
                distance: 10.0,
                index: i as u32,
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

/// A single blur dab straddling the hard edge averages white and black
/// into an intermediate gray at the edge, and leaves pixels well outside
/// the dab footprint untouched.
#[test]
fn blur_dab_softens_hard_edge_to_gray() {
    // Radius ≈ 0.15 * 256 ≈ 38 px — wide enough that the kernel at the
    // edge spans both the white and black halves.
    let rgba = render_blur_dabs(0.15, 1.0, 1.0, &[[EDGE_X as f32, 64.0]]);

    let centre = pixel(&rgba, EDGE_X, 64);
    assert!(
        centre[0] > 30 && centre[0] < 225,
        "a blur dab straddling a hard black/white edge should leave an \
         intermediate gray at the edge; got {centre:?}"
    );

    // Far inside the white half and the black half, outside the dab
    // footprint — must be untouched.
    let far_white = pixel(&rgba, 8, 8);
    let far_black = pixel(&rgba, CANVAS - 8, CANVAS - 8);
    assert_eq!(
        far_white,
        [255, 255, 255, 255],
        "white pixel outside the dab footprint must be untouched"
    );
    assert_eq!(
        far_black,
        [0, 0, 0, 255],
        "black pixel outside the dab footprint must be untouched"
    );
}

/// **Dwell-compounding test.** With `opacity < 1`, each pass blends only
/// a fraction of the blurred result, so a second overlapping dab — which
/// reads the *cumulative* scratch through the per-dab read-mirror barrier
/// — pushes the edge centre further toward gray than one pass does.
///
/// Working barrier: pass 2 reads pass 1's grayer writeback and softens
/// further, so the (initially black) edge centre rises more.
///
/// Broken barrier (e.g. all dabs collapsed to one instanced draw): pass 2
/// would re-read the pre-stroke black and land at the same value as pass 1.
#[test]
fn blur_second_pass_compounds() {
    let centre = [EDGE_X as f32, 64.0];
    // opacity 0.5 → each pass blends 50% of the blurred neighborhood, so
    // the compounding is visible. The edge centre starts black (0) and
    // rises toward the ~mid-gray neighborhood average with each pass.
    let one_pass = render_blur_dabs(0.15, 1.0, 0.5, &[centre]);
    let two_pass = render_blur_dabs(0.15, 1.0, 0.5, &[centre, centre]);

    let v1 = pixel(&one_pass, EDGE_X, 64)[0];
    let v2 = pixel(&two_pass, EDGE_X, 64)[0];
    assert!(
        v2 > v1 + 8,
        "a second overlapping blur pass must compound (read the first \
         pass's writeback through the per-dab barrier and soften further): \
         one pass left {v1}, two passes left {v2}"
    );
}
