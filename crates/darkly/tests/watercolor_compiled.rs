//! Smoke tests for the watercolor brushes after migration to the
//! compiled `watercolor_compiled` terminal. Each test loads the actual
//! builtin graph, renders a couple of dabs over a non-empty
//! pre_stroke, and checks the watercolor blend deposits something
//! reasonable. The pickup atlas pass + per-brush compiled composite
//! pass are exercised end-to-end.

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

/// Light blue initial canvas (alpha = 1.0) so pickup has something to
/// mix into the load. Watercolor's `mix(canvas_rgb, fg_color.rgb,
/// deposit)` blends pre_stroke pixels with the brush color.
fn light_blue_canvas() -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px[0] = 100;
        px[1] = 150;
        px[2] = 230;
        px[3] = 255;
    }
    out
}

fn render_dabs(
    brush_name: &str,
    size_override: f32,
    color: [f32; 4],
    dabs: &[(f32, f32)],
) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == brush_name)
        .unwrap_or_else(|| panic!("builtin brush `{brush_name}` not registered"));

    let mut graph = brush.metadata.graph.clone();
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "watercolor_compiled")
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("brush `{brush_name}` must terminate in watercolor_compiled"));
    graph
        .set_port_default(term_id, "size", size_override)
        .unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, CANVAS, CANVAS, &light_blue_canvas());
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
        label: Some("watercolor-compiled-test-pre-stroke"),
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
        let mut ctx = make_ctx!("watercolor-compiled-test-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("watercolor-compiled-test-flush");
        for (i, (x, y)) in dabs.iter().enumerate() {
            let info = PaintInformation {
                pos: [*x, *y],
                pressure: 1.0,
                ..Default::default()
            };
            runner.seed_sensors(&info, color, 0xC0FFEE, i as u32);
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
