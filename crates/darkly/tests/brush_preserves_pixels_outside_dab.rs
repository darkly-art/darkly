//! Universal commit invariant: a pixel that lies outside every dab's
//! footprint must survive the stroke unchanged — regardless of brush.
//!
//! Iterates over every builtin brush. Regression for the watercolor
//! commit path, which used to seed its scratch with a straight-alpha
//! copy of pre_stroke and then re-source-over the whole scratch onto
//! pre_stroke as if it were premultiplied — boosting alpha and rgb on
//! every partial-alpha pixel the moment a stroke began.
//!
//! Run with a small dab at the canvas centre so the corner pixels are
//! far outside the bbox of any brush we have today.

use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
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

fn solid_canvas(rgba: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&rgba);
    }
    out
}

fn render_one_dab(brush_name: &str, color: [f32; 4], canvas: &[u8]) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == brush_name)
        .unwrap_or_else(|| panic!("builtin brush `{brush_name}` not registered"));

    let mut graph = brush.metadata.graph.clone();
    let term_id = darkly::brush::find_terminal(&graph)
        .unwrap_or_else(|err| panic!("brush `{brush_name}`: {err}"));
    // Small size keeps the dab footprint well clear of the corner pixel.
    graph.set_port_default(term_id, "size", 0.05).unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) = create_test_texture(&device, &queue, CANVAS, CANVAS, canvas);
    let pipelines = BrushPipelines::new(&device, &queue);
    let mut stroke_buffer = StrokeBuffer::new(&device, CANVAS, CANVAS, &pipelines);

    let pre_stroke = darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        CANVAS,
        CANVAS,
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("brush-untouched-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke);
    queue.submit([enc.finish()]);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");
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
                blend_mode: 0,
                preview_mask_view: None,
                preview_mask_size: (0, 0),
                preview_mask_overlay: None,
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
        let mut ctx = make_ctx!("brush-untouched-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("brush-untouched-flush");
        let info = PaintInformation {
            pos: [64.0, 64.0],
            pressure: 1.0,
            ..Default::default()
        };
        runner.seed_sensors(&info, color, 0xC0FFEE, 0);
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
        CANVAS,
        CANVAS,
    )
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * CANVAS + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

#[test]
fn every_builtin_brush_preserves_pixels_outside_dab_on_partial_alpha_layer() {
    let initial = [100u8, 150, 230, 128];
    let canvas = solid_canvas(initial);

    let brushes = darkly::brush::builtin_brushes::all();
    assert!(!brushes.is_empty(), "no builtin brushes registered");

    for brush in &brushes {
        let name = &brush.metadata.name;
        let rgba = render_one_dab(name, [1.0, 0.0, 0.0, 1.0], &canvas);
        // Corner pixel — distance >85 px from the dab centre at (64, 64),
        // well outside the bbox of any builtin brush at size=0.05.
        let corner = pixel(&rgba, 2, 2);
        assert_eq!(
            corner, initial,
            "brush `{name}`: partial-alpha pixel outside the dab footprint \
             must be unchanged, got {corner:?}",
        );
    }
}
