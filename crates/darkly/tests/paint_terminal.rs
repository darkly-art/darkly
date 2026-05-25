//! Correctness tests for the `paint` terminal — the single-pass instanced
//! fragment terminal that drives the Basic brushes (Round, Airbrush,
//! Ink Pen). See `crates/darkly/src/brush/nodes/paint.rs` and
//! `paint-compute-perf-tracking.md` attempt #4 for the terminal's
//! design and motivation.
//!
//! The hot-path performance properties belong to
//! `bench-results/stroke-replay-matrix-paint-*`. This file owns the
//! correctness invariants: that the new terminal's pixel output matches
//! what `paint_compute` produced byte-for-byte (within rgba8
//! quantisation), and that erase + ordering work.

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
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
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

fn test_engine(w: u32, h: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, w, h)
}

/// Stabilizer-free Ink-Pen-style graph driving the `paint` terminal —
/// one event places one dab at the requested position, no smoothing.
fn paint_no_stabilize() -> Graph<BrushWireType> {
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
        "paint",
        registry.get("paint").unwrap().ports.clone(),
        vec![],
    );

    let wires = [
        (pen, "position", terminal, "position"),
        (pen, "pressure", terminal, "size_input"),
        (pen, "pressure", terminal, "flow"),
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

/// Same convention as the predecessor `paint_compute_alpha` test: a
/// half-flow white dab must read as white at partial alpha (R=G=B=255),
/// not grey at partial alpha (R=G=B=A). Catches the dark-edge artifact
/// that would land if the new fragment terminal's premultiplied output
/// were ever mis-blitted to the layer with `fg_premultiplied: false`.
#[test]
fn paint_half_flow_paints_white_not_grey() {
    let (w, h) = (CANVAS, CANVAS);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    let graph = paint_no_stabilize();
    let json = serde_json::to_string(&graph).expect("graph serializes");
    engine
        .set_brush_graph(&json)
        .expect("graph compiles as a brush");

    let cx = (w / 2) as f32;
    let cy = (h / 2) as f32;

    let stroke_at = |time_ms: f64| StrokeOp::BrushStroke {
        x: cx,
        y: cy,
        pressure: 0.5,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms,
        cr: 1.0,
        cg: 1.0,
        cb: 1.0,
        ca: 1.0,
    };
    engine.begin_stroke(layer_id);
    engine.stroke_to(stroke_at(0.0));
    engine.stroke_to(stroke_at(16.0));
    engine.end_stroke();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer_id);
    let idx = ((cy as u32 * w + cx as u32) * 4) as usize;
    let (r, g, b, a) = (
        pixels[idx] as i32,
        pixels[idx + 1] as i32,
        pixels[idx + 2] as i32,
        pixels[idx + 3] as i32,
    );

    assert!(
        a > 0,
        "centre pixel should have some alpha after painting, got rgba={:?}",
        (r, g, b, a),
    );
    let tol = 3;
    assert!(
        (r - 255).abs() <= tol && (g - 255).abs() <= tol && (b - 255).abs() <= tol,
        "centre RGB must read pure white (≈255) after the paint terminal's premultiplied \
         output is correctly composited; got rgba={:?}",
        (r, g, b, a),
    );
}

// ── Direct-graph harness: stamps multiple dabs in one ctx so we can hit
//    `flush_dabs` deterministically, like `paint_compute_order` does. ────

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

fn paint_graph(size: f32, softness: f32) -> Graph<BrushWireType> {
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
        "paint",
        registry.get("paint").unwrap().ports.clone(),
        vec![],
    );
    graph.set_port_default(terminal, "size", size).unwrap();
    graph.set_port_default(terminal, "size_input", 1.0).unwrap();
    graph
        .set_port_default(terminal, "softness", softness)
        .unwrap();
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

fn harness(size: f32, softness: f32) -> Harness {
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
        label: Some("paint-terminal-pre-stroke-init"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke_paint_target);
    queue.submit([enc.finish()]);

    let graph = paint_graph(size, softness);
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
    ($h:ident, $label:expr, $blend_mode:expr) => {{
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
            blend_mode: $blend_mode,
            preview_mask_view: None,
            preview_mask_size: (0, 0),
            brush_preview_info: None,
            pre_stroke_texture: Some(_pre_stroke_texture),
            pre_stroke_bind_group: Some(_pre_stroke_bind_group),
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

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * CANVAS + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Hardware source-over blends instances in primitive-issue order per the
/// WebGPU spec. Four fully-opaque overlapping dabs in colour order must
/// leave the centre reading as the *last* dab. Anything else means we're
/// blending the wrong direction (or the order isn't honoured).
#[test]
fn paint_dabs_blend_in_instance_order() {
    let mut h = harness(0.5, 0.0);
    let cx = (CANVAS / 2) as f32;
    let cy = (CANVAS / 2) as f32;

    {
        let mut ctx = make_ctx!(h, "paint-order-begin", 0u32);
        h.runner.begin_stroke(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }

    let colors: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 1.0], // red
        [0.0, 1.0, 0.0, 1.0], // green
        [0.0, 0.0, 1.0, 1.0], // blue
        [1.0, 1.0, 1.0, 1.0], // white
    ];

    {
        let mut ctx = make_ctx!(h, "paint-order-dispatch", 0u32);
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
        assert_eq!(ctx.pending_dab_count, 4);
        h.runner.flush_dabs(&mut ctx);
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
    let tol = 2;
    assert!(
        (r as i32 - 255).abs() <= tol
            && (g as i32 - 255).abs() <= tol
            && (b as i32 - 255).abs() <= tol
            && (a as i32 - 255).abs() <= tol,
        "centre pixel must read the last-queued (white) dab's colour, got rgba=({r},{g},{b},{a})",
    );
}

/// Erase pipeline: paint solid white at the centre, then erase with a
/// fully-opaque dab at the same spot. After the erase phase, the centre
/// alpha must drop to zero (or near-zero through rgba8 quantisation).
#[test]
fn paint_erase_reduces_alpha() {
    let mut h = harness(0.5, 0.0);
    let cx = (CANVAS / 2) as f32;
    let cy = (CANVAS / 2) as f32;

    // Phase 1 — begin + one solid white dab + commit (paint mode).
    {
        let mut ctx = make_ctx!(h, "paint-erase-paint-begin", 0u32);
        h.runner.begin_stroke(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!(h, "paint-erase-paint-dispatch", 0u32);
        let info = PaintInformation {
            pos: [cx, cy],
            pressure: 1.0,
            index: 0,
            ..PaintInformation::default()
        };
        h.runner.clear_slots();
        h.runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0, 0);
        h.runner.execute_cpu();
        h.runner.execute_gpu(&mut ctx);
        h.runner.flush_dabs(&mut ctx);
        h.runner.commit(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }
    // Sanity: layer is opaque white at centre.
    let after_paint = readback_texture(
        &h.device,
        &h.queue,
        &h.layer_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        CANVAS,
        CANVAS,
    );
    let [_, _, _, a_paint] = pixel(&after_paint, cx as u32, cy as u32);
    assert!(
        a_paint > 200,
        "phase 1 should leave the centre opaque; got alpha={a_paint}",
    );

    // Phase 2 — fresh stroke in erase mode at the same position.
    // begin_stroke clears the scratch; the erase pipeline reduces the
    // layer's existing alpha by the dab coverage at commit time.
    {
        let mut ctx = make_ctx!(h, "paint-erase-erase-begin", 1u32);
        h.runner.begin_stroke(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!(h, "paint-erase-erase-dispatch", 1u32);
        let info = PaintInformation {
            pos: [cx, cy],
            pressure: 1.0,
            index: 0,
            ..PaintInformation::default()
        };
        h.runner.clear_slots();
        h.runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 1, 0);
        h.runner.execute_cpu();
        h.runner.execute_gpu(&mut ctx);
        h.runner.flush_dabs(&mut ctx);
        h.runner.commit(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }
    let after_erase = readback_texture(
        &h.device,
        &h.queue,
        &h.layer_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        CANVAS,
        CANVAS,
    );
    let [_, _, _, a_erase] = pixel(&after_erase, cx as u32, cy as u32);
    assert!(
        a_erase < a_paint,
        "erase phase must reduce alpha at the dab centre: paint_alpha={a_paint} erase_alpha={a_erase}",
    );
}

/// Two well-separated opaque dabs in one flush must leave BOTH dab
/// positions opaque and the gap between them transparent. Catches a
/// vertex-shader bug where per-instance data leaks across instances
/// (e.g. an off-by-one indexing the dabs storage buffer).
#[test]
fn paint_stroke_shape_two_separated_dabs() {
    // `size * SIZE_REFERENCE_PX (512) * 0.5` is the radius in px. With
    // size=0.05 → radius ≈ 13 px, well clear of the 32-px gap between
    // the two dab centres on the 128-canvas.
    let mut h = harness(0.05, 0.0);
    let cy = (CANVAS / 2) as f32;
    let x_left = (CANVAS / 4) as f32;
    let x_right = (CANVAS * 3 / 4) as f32;

    {
        let mut ctx = make_ctx!(h, "paint-shape-begin", 0u32);
        h.runner.begin_stroke(&mut ctx);
        h.queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!(h, "paint-shape-dispatch", 0u32);
        for (i, &x) in [x_left, x_right].iter().enumerate() {
            let info = PaintInformation {
                pos: [x, cy],
                pressure: 1.0,
                index: i as u32,
                ..PaintInformation::default()
            };
            h.runner.clear_slots();
            h.runner
                .seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0, i as u32);
            h.runner.execute_cpu();
            h.runner.execute_gpu(&mut ctx);
        }
        h.runner.flush_dabs(&mut ctx);
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
    let [_, _, _, a_left] = pixel(&pixels, x_left as u32, cy as u32);
    let [_, _, _, a_right] = pixel(&pixels, x_right as u32, cy as u32);
    let [_, _, _, a_gap] = pixel(&pixels, CANVAS / 2, cy as u32);

    assert!(
        a_left > 200,
        "left dab centre should be opaque, got alpha={a_left}"
    );
    assert!(
        a_right > 200,
        "right dab centre should be opaque, got alpha={a_right}"
    );
    assert!(
        a_gap < 16,
        "gap between dabs should remain transparent, got alpha={a_gap} \
         (per-instance vertex data may be leaking across draws)",
    );
}
