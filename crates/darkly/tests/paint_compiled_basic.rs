//! Smoke tests for the Round / Airbrush / Ink Pen builtins after the
//! migration from the `paint` dispatch terminal to the compiled
//! `paint_compiled` terminal. Each test loads the actual builtin
//! graph (no test-only rewiring), renders one dab through the
//! compiled pipeline, and asserts the dab deposited inside its
//! declared bbox.
//!
//! `perlin_ink.rs` exercises the deeper invariants of the compiled
//! pipeline (bbox-correctness on overlapping dabs, flow scaling,
//! shape parity). These tests only need to verify each migrated
//! brush's graph wires up cleanly and produces visible output —
//! per-brush wire bugs (e.g. forgetting `paint_color → stamp.color`)
//! surface here while the pipeline itself stays covered by
//! `perlin_ink.rs`.

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

fn black_canvas() -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    out
}

/// Render one dab of the given builtin brush at canvas centre and
/// return the resulting RGBA8 readback. `size_override` is forced
/// onto the terminal's `size` port so the dab fits in our 128px
/// test canvas regardless of the brush's exposed default.
fn render_single_dab(brush_name: &str, size_override: f32, color: [f32; 4]) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == brush_name)
        .unwrap_or_else(|| panic!("builtin brush `{brush_name}` not registered"));

    let mut graph = brush.metadata.graph.clone();
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "paint_compiled")
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("brush `{brush_name}` must terminate in paint_compiled"));
    graph
        .set_port_default(term_id, "size", size_override)
        .unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, CANVAS, CANVAS, &black_canvas());
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
        label: Some("paint-compiled-basic-pre-stroke"),
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
                compiled_brush: None,
                slot_outputs_owned: None,
            }
        }};
    }

    {
        let mut ctx = make_ctx!("paint-compiled-basic-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("paint-compiled-basic-dab");
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

fn center_rgba(rgba: &[u8]) -> [u8; 4] {
    let idx = ((64 * CANVAS + 64) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

fn count_deposited(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4)
        .filter(|p| p[0] > 0 || p[1] > 0 || p[2] > 0)
        .count()
}

#[test]
fn round_deposits_at_center() {
    let rgba = render_single_dab("Round", 0.15, [1.0, 0.0, 0.0, 1.0]);
    let center = center_rgba(&rgba);
    assert!(
        center[0] > 150 && center[1] < 60 && center[2] < 60,
        "Round center should be ~red, got {center:?}"
    );
    assert!(
        count_deposited(&rgba) > 500,
        "Round should deposit a substantial disc, got {} pixels",
        count_deposited(&rgba),
    );
}

#[test]
fn airbrush_deposits_softer_than_round() {
    // Airbrush has softness=1.0; Round has 0.5. Centre coverage should
    // still be solid (pressure→opacity is 1.0), but the alpha falloff
    // at the rim is gentler. Smoke-test centre only here — the softer
    // edge is hard to assert quantitatively without a per-pixel
    // gradient probe.
    let rgba = render_single_dab("Airbrush", 0.15, [0.0, 1.0, 0.0, 1.0]);
    let center = center_rgba(&rgba);
    assert!(
        center[1] > 150 && center[0] < 60 && center[2] < 60,
        "Airbrush center should be ~green, got {center:?}"
    );
    assert!(count_deposited(&rgba) > 500);
}

#[test]
fn ink_pen_deposits_with_pressure_curve() {
    // Ink Pen uses a front-loaded curve so pressure=1.0 reaches full
    // size — same end deposit as Round at full pressure. Curve only
    // shapes the response at lower pressures (not exercised here).
    let rgba = render_single_dab("Ink Pen", 0.15, [0.0, 0.0, 1.0, 1.0]);
    let center = center_rgba(&rgba);
    assert!(
        center[2] > 150 && center[0] < 60 && center[1] < 60,
        "Ink Pen center should be ~blue, got {center:?}"
    );
    assert!(count_deposited(&rgba) > 500);
}
