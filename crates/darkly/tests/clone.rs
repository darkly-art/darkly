//! GPU integration tests for the Clone brush.
//!
//! The Clone brush reuses the `paint` terminal but sources each
//! fragment's colour from the frozen pre-stroke snapshot at a
//! stroke-constant offset (`clone_source` node). These tests paint a
//! known source patch, freeze it, set the clone anchors, stroke at a
//! destination offset, and assert the destination reads back as the
//! source colour shifted by the offset.
//!
//! The frame conversion (`uv = (src - layer_offset) / layer_size`) is
//! the recurring hazard, so the copy is exercised both at the origin and
//! under a **non-zero canvas origin + offset layer**. Erase mode is
//! covered too — clone inherits the terminal's paint-vs-erase commit.

use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::eval::{BrushGraphRunner, CloneState};
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, StrokeResources};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::coord::CanvasRect;
use darkly::gpu::paint_target::GpuPaintTarget;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};

const W: u32 = 128;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

/// Layer content in layer-local pixels: left half opaque red, right half
/// opaque black. The clone samples the red half and deposits it onto the
/// black half; erase removes the opaque black.
fn split_red_black() -> Vec<u8> {
    let mut out = vec![0u8; (W * W * 4) as usize];
    for y in 0..W {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            if x < W / 2 {
                out[i] = 255; // red
            }
            out[i + 3] = 255; // opaque
        }
    }
    out
}

struct CloneParams {
    /// Plane offset of the canvas window and the layer (kept equal so the
    /// window == layer; a non-zero value crops the plane).
    origin: [i32; 2],
    /// Clone source anchor in plane pixels.
    source_anchor: [f32; 2],
    /// First-dab destination anchor in plane pixels (also the dab centre).
    dest: [f32; 2],
    erase: bool,
}

/// Drive one clone dab through the runner directly (bypassing the stroke
/// engine so the anchors are set explicitly) and return the layer readback
/// in layer-local pixels.
fn render_clone(p: &CloneParams) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Clone")
        .expect("Clone builtin registered");

    let mut graph = brush.metadata.graph.clone();
    let term_id = darkly::brush::find_terminal(&graph).expect("clone has a terminal");
    // Force a size that fits inside the half-canvas patch so the whole
    // dab footprint samples red.
    graph.set_port_default(term_id, "size", 0.15).unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, W, W, &split_red_black());
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut stroke_buffer = StrokeBuffer::new(&device, W, W, &pipelines);

    let layer_rect = CanvasRect::from_xywh(p.origin[0], p.origin[1], W, W);
    let pre_stroke = GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        layer_rect,
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("clone-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke);
    queue.submit([enc.finish()]);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("clone compiles");
    assert!(
        runner.samples_source(),
        "Clone brush must report samples_source"
    );
    runner.set_clone_state(Some(CloneState {
        source_anchor: p.source_anchor,
        dest_anchor: p.dest,
    }));

    let blend_mode = u32::from(p.erase);
    macro_rules! make_ctx {
        ($label:expr) => {{
            let (scratch, pre_stroke_tex, pre_stroke_bg) = stroke_buffer.parts_for_brush_ctx();
            BrushGpuContext {
                encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some($label),
                }),
                device: &device,
                queue: &queue,
                pipelines: &pipelines,
                selection_bind_group: pipelines.default_selection_bind_group(),
                canvas_width: W,
                canvas_height: W,
                canvas_origin: p.origin,
                blend_mode,
                view_rotation: 0.0,
                perf: BrushPerfCounters::default(),
                stroke: Some(StrokeResources {
                    scratch,
                    paint_target: GpuPaintTarget::from_canvas_texture(
                        &layer_texture,
                        &layer_view,
                        wgpu::TextureFormat::Rgba8Unorm,
                        layer_rect,
                    ),
                    pre_stroke_texture: pre_stroke_tex,
                    pre_stroke_bind_group: pre_stroke_bg,
                }),
                preview: None,
                dab_batch: DabBatch::default(),
            }
        }};
    }

    {
        let mut ctx = make_ctx!("clone-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("clone-dab");
        let info = PaintInformation {
            pos: p.dest,
            pressure: 1.0,
            ..Default::default()
        };
        runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0xC10E, 0);
        runner.execute_cpu();
        runner.execute_gpu(&mut ctx);
        runner.flush_dabs(&mut ctx);
        runner.commit(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }

    readback_texture(
        &device,
        &queue,
        &layer_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        W,
        W,
    )
}

/// Read a pixel at layer-local `(x, y)`.
fn local_px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn clone_copies_source_onto_destination() {
    // Source at (32, 64) in the red half; destination at (96, 64) in the
    // black half. Aligned mode, offset = source − dest = (−64, 0).
    let rgba = render_clone(&CloneParams {
        origin: [0, 0],
        source_anchor: [32.0, 64.0],
        dest: [96.0, 64.0],
        erase: false,
    });
    // Before clone the destination was black; after, it must read red.
    let dst = local_px(&rgba, 96, 64);
    assert!(
        dst[0] > 150 && dst[1] < 60 && dst[2] < 60,
        "cloned destination should be red (copied from the source patch), got {dst:?}"
    );
}

#[test]
fn clone_copies_under_nonzero_origin_and_offset_layer() {
    // Same geometry lifted into a cropped plane: the canvas window and the
    // layer both sit at plane (10, 20). This exercises the
    // `uv = (src - layer_offset) / layer_size` conversion — the recurring
    // wrong-frame hazard. Source/dest are given in plane pixels; the
    // readback is layer-local, so subtract the offset when probing.
    let origin = [10, 20];
    let rgba = render_clone(&CloneParams {
        origin,
        source_anchor: [10.0 + 32.0, 20.0 + 64.0],
        dest: [10.0 + 96.0, 20.0 + 64.0],
        erase: false,
    });
    let dst = local_px(&rgba, 96, 64);
    assert!(
        dst[0] > 150 && dst[1] < 60 && dst[2] < 60,
        "offset-layer clone destination should be red, got {dst:?}"
    );
}

#[test]
fn clone_erase_removes_at_destination() {
    // Erase mode: the clone coverage drives a destination-out commit, so
    // the opaque-black destination is removed (alpha drops toward 0).
    let rgba = render_clone(&CloneParams {
        origin: [0, 0],
        source_anchor: [32.0, 64.0],
        dest: [96.0, 64.0],
        erase: true,
    });
    let dst = local_px(&rgba, 96, 64);
    assert!(
        dst[3] < 40,
        "erase-mode clone should remove the destination pixel (alpha≈0), got {dst:?}"
    );
}
