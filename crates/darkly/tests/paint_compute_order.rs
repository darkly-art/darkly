//! Regression test for the **per-thread dab-loop ordering** invariant in
//! `paint_compute.wgsl`.
//!
//! After the thread-per-pixel rework, each compute thread owns one pixel and
//! walks the queued dab list serially in array order. Cross-dab compositing
//! ordering is intrinsic to that loop — there is no inter-dispatch
//! synchronization to worry about because the whole phase is one dispatch.
//!
//! This test queues four fully-opaque dabs of distinct colors at the same
//! canvas position into a single `BrushGpuContext`, calls `flush_compute`
//! once, and asserts the centre pixel reads as the LAST dab's color. Under
//! source-over with `src.a == 1.0`, later dabs fully cover earlier ones —
//! anything but the last color means the per-thread loop didn't iterate
//! the queue in array order.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::dab_pool::DabTexturePool;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::brush::wire::{BrushWireType, TextureHandle};
use darkly::brush::BrushNodeRegistry;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};
use darkly::nodegraph::{Graph, PortRef};

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

/// pen_input → paint_compute, with `paint_color → terminal.color`. Mirrors
/// the no-stabilize Ink Pen graph used in `paint_compute_alpha.rs`. Size is
/// pinned via a port default so each test dab covers the same disc.
fn paint_compute_graph(size: f32) -> Graph<BrushWireType> {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let terminal = graph.add_node(
        "paint_compute",
        registry.get("paint_compute").unwrap().ports.clone(),
        vec![],
    );

    graph.set_port_default(terminal, "size", size).unwrap();
    graph.set_port_default(terminal, "size_input", 1.0).unwrap();
    graph.set_port_default(terminal, "softness", 0.0).unwrap();
    graph.set_port_default(terminal, "flow", 1.0).unwrap();
    graph.set_port_default(terminal, "opacity", 1.0).unwrap();

    let wires = [
        (pen, "position", terminal, "position"),
        (paint_color, "color", terminal, "color"),
    ];
    for (fnode, fport, tnode, tport) in wires {
        graph
            .connect(
                PortRef {
                    node: fnode,
                    port: fport.into(),
                },
                PortRef {
                    node: tnode,
                    port: tport.into(),
                },
            )
            .unwrap();
    }

    graph
}

struct Harness {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    layer_texture: wgpu::Texture,
    layer_view: wgpu::TextureView,
    pipelines: BrushPipelines,
    dab_pool: DabTexturePool,
    stroke_buffer: StrokeBuffer,
    runner: BrushGraphRunner,
    resource_handles: HashMap<String, TextureHandle>,
}

fn harness(size: f32) -> Harness {
    let (device, queue) = shared_device();
    let initial = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, CANVAS, CANVAS, &initial);

    let dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout());

    let stroke_buffer = StrokeBuffer::new(
        &device,
        CANVAS,
        CANVAS,
        dab_pool.bind_group_layout(),
        &pipelines,
    );

    let pre_stroke_paint_target = darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        CANVAS,
        CANVAS,
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("paint-compute-order-pre-stroke-init"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke_paint_target);
    queue.submit([enc.finish()]);

    let graph = paint_compute_graph(size);
    let runner = compile_graph(&graph).expect("graph compiles");

    Harness {
        device,
        queue,
        layer_texture,
        layer_view,
        pipelines,
        dab_pool,
        stroke_buffer,
        runner,
        resource_handles: HashMap::new(),
    }
}

macro_rules! make_ctx {
    ($h:ident, $label:expr) => {{
        let (_scratch, _pre_stroke_texture, _pre_stroke_bind_group) =
            $h.stroke_buffer.parts_for_brush_ctx();
        BrushGpuContext {
            encoder: $h
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some($label),
                }),
            device: &$h.device,
            queue: &$h.queue,
            dab_pool: &mut $h.dab_pool,
            pipelines: &$h.pipelines,
            scratch: Some(_scratch),
            canvas_width: CANVAS,
            canvas_height: CANVAS,
            paint_target: Some(
                darkly::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
                    &$h.layer_texture,
                    &$h.layer_view,
                    wgpu::TextureFormat::Rgba8Unorm,
                    CANVAS,
                    CANVAS,
                ),
            ),
            selection_bind_group: $h.pipelines.default_selection_bind_group(),
            preview_target_view: None,
            resource_handles: &$h.resource_handles,
            blend_mode: 0,
            preview_mask_view: None,
            preview_mask_size: (0, 0),
            brush_preview_info: None,
            pre_stroke_texture: Some(_pre_stroke_texture),
            pre_stroke_bind_group: Some(_pre_stroke_bind_group),
            dab_write_canvas_bbox: None,
            perf: BrushPerfCounters::default(),
            pending_compute_dab_bytes: Vec::new(),
            pending_compute_dab_count: 0,
            pending_dabs_bbox: None,
        }
    }};
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * CANVAS + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn paint_compute_dabs_composite_in_queue_order() {
    // Big enough dab that all four queued centres land squarely on the
    // centre pixel — a single-pixel difference between dab positions still
    // leaves the centre fully covered by each disc.
    let mut h = harness(0.5);
    let cx = (CANVAS / 2) as f32;
    let cy = (CANVAS / 2) as f32;

    // begin_stroke once — sets up the scratch & compute buffer.
    {
        let mut ctx = make_ctx!(h, "paint-compute-order-begin");
        h.runner.begin_stroke(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }

    // Queue four dabs of distinct fully-opaque colors at (essentially) the
    // same centre. Sub-pixel offsets keep the dab dispatcher's spatial
    // bookkeeping honest while guaranteeing the centre pixel sits inside
    // every disc.
    //
    // We reuse ONE BrushGpuContext across all four `execute_gpu` calls so
    // all four dab records accumulate in `pending_compute_dab_bytes`; only
    // then is `flush_compute` invoked, exercising the per-thread loop's
    // ordering invariant.
    let colors: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 1.0], // red
        [0.0, 1.0, 0.0, 1.0], // green
        [0.0, 0.0, 1.0, 1.0], // blue
        [1.0, 1.0, 1.0, 1.0], // white
    ];

    {
        let mut ctx = make_ctx!(h, "paint-compute-order-dispatch");

        for (i, color) in colors.iter().enumerate() {
            let info = PaintInformation {
                pos: [cx + i as f32 * 0.25, cy + i as f32 * 0.25],
                pressure: 1.0,
                index: i as u32,
                ..PaintInformation::default()
            };
            h.runner.clear_slots();
            h.runner.seed_sensors(&info, *color, 0, info.index);
            h.runner.execute_cpu();
            h.runner.execute_gpu(&mut ctx);
        }

        assert_eq!(
            ctx.pending_compute_dab_count, 4,
            "expected four dabs queued before flush, got {}",
            ctx.pending_compute_dab_count,
        );

        h.runner.flush_compute(&mut ctx);
        h.runner.commit(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }

    let pixels = readback_texture(
        &h.device,
        &h.queue,
        &h.layer_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        CANVAS,
        CANVAS,
    );
    let [r, g, b, a] = pixel(&pixels, cx as u32, cy as u32);
    // After source-over of four fully-opaque dabs, the latest dab (white)
    // must dominate. Anything else means the per-thread loop iterated the
    // dab queue out of order. Tolerance ±2 covers rgba8 quantisation
    // through the unpack/pack round trip.
    let tol = 2;
    assert!(
        (r as i32 - 255).abs() <= tol
            && (g as i32 - 255).abs() <= tol
            && (b as i32 - 255).abs() <= tol
            && (a as i32 - 255).abs() <= tol,
        "centre pixel must be the last-queued dab's colour (white), got rgba=({r}, {g}, {b}, {a})",
    );
}
