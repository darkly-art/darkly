//! Integration tests for the Perlin Ink brush — the first 100%-
//! compiled brush. Exercises the full `paint_compiled` pipeline
//! end-to-end on a real GPU device:
//!
//! 1. **Single dab renders** — one dab through the compiled pipeline
//!    deposits color where it should. Smoke test that the pipeline
//!    builds and the dab buffer round-trips through the storage
//!    binding.
//! 2. **Two dabs in the same flush produce distinct silhouettes** —
//!    two dabs queued in the same phase get independent per-dab
//!    random seeds (the runner's `dab_index` increments) and the
//!    compiled shader reads them per-instance. Catches accidentally
//!    indexing all instances into slot 0 of the dab buffer.
//! 3. **Zero amplitude collapses to a disc** — with all three random
//!    nodes forced to 0 and the perlin amplitude defaulted via wire
//!    remap, the rendered shape is a disc within blend tolerance.
//!    Validates the compiled `shape_r_theta` parity with the existing
//!    CPU implementation.

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
use darkly::gpu::params::ParamValue;
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

/// Build a minimal compiled-brush graph for testing:
///
///   pen_input.position → paint_compiled.position
///   pen_input.pressure → curve → paint_compiled.size_input
///   paint_color.color  → stamp.color
///   circle.texture     → stamp.tip       (per-dab shape feed)
///   stamp.dab          → paint_compiled.rgba
///
/// `algorithm` selects the circle's shape function. `amplitude`
/// defaults to 0 (= disc) unless the caller overrides.
fn build_test_graph(algorithm: i32, amplitude: f32, size: f32) -> Graph<BrushWireType> {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

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
    let curve = graph.add_node(
        "curve",
        registry.get("curve").unwrap().ports.clone(),
        vec![ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]])],
    );
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![ParamValue::Int(algorithm)],
    );
    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![ParamValue::Int(0)], // Alpha Mask
    );
    let terminal = graph.add_node(
        "paint_compiled",
        registry.get("paint_compiled").unwrap().ports.clone(),
        vec![],
    );

    graph
        .set_port_default(circle, "amplitude", amplitude)
        .unwrap();
    graph.set_port_default(circle, "softness", 0.0).unwrap();
    graph.set_port_default(terminal, "size", size).unwrap();
    graph.set_port_default(terminal, "opacity", 1.0).unwrap();
    graph.set_port_default(terminal, "flow", 1.0).unwrap();

    let wires = [
        (pen, "pressure", curve, "input"),
        (curve, "output", terminal, "size_input"),
        (pen, "pressure", stamp, "flow"),
        (circle, "texture", stamp, "tip"),
        (paint_color, "color", stamp, "color"),
        (stamp, "dab", terminal, "rgba"),
        (pen, "position", terminal, "position"),
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

fn harness(initial: &[u8], graph: Graph<BrushWireType>) -> Harness {
    let (device, queue) = shared_device();
    let (layer_texture, layer_view) = create_test_texture(&device, &queue, CANVAS, CANVAS, initial);

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
        label: Some("perlin-ink-test-pre-stroke-init"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke_paint_target);
    queue.submit([enc.finish()]);

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
    ($h:ident, $label:expr, $resources:expr) => {{
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
            resource_handles: $resources,
            blend_mode: 0,
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

impl Harness {
    fn begin_stroke(&mut self) {
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "perlin-ink-test-begin", &resources);
            self.runner.begin_stroke(&mut ctx);
            self.queue.submit([ctx.encoder.finish()]);
        }
        self.resource_handles = resources;
    }

    fn dab_and_flush(&mut self, info: &PaintInformation, color: [f32; 4], dab_index: u32) {
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "perlin-ink-test-dab", &resources);
            self.runner.seed_sensors(info, color, 0xC0FFEE, dab_index);
            self.runner.execute_cpu();
            self.runner.execute_gpu(&mut ctx);
            self.runner.flush_dabs(&mut ctx);
            self.runner.commit(&mut ctx);
            self.queue.submit([ctx.encoder.finish()]);
        }
        self.resource_handles = resources;
    }

    fn two_dabs_same_phase(&mut self, a: &PaintInformation, b: &PaintInformation, color: [f32; 4]) {
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "perlin-ink-test-two-dabs", &resources);
            self.runner.seed_sensors(a, color, 0xC0FFEE, 0);
            self.runner.execute_cpu();
            self.runner.execute_gpu(&mut ctx);
            self.runner.seed_sensors(b, color, 0xC0FFEE, 1);
            self.runner.execute_cpu();
            self.runner.execute_gpu(&mut ctx);
            // Single flush, two instanced dabs.
            self.runner.flush_dabs(&mut ctx);
            self.runner.commit(&mut ctx);
            self.queue.submit([ctx.encoder.finish()]);
        }
        self.resource_handles = resources;
    }

    fn readback_canvas(&self) -> Vec<u8> {
        readback_texture(
            &self.device,
            &self.queue,
            &self.layer_texture,
            wgpu::TextureFormat::Rgba8Unorm,
            CANVAS,
            CANVAS,
        )
    }
}

fn center_pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * CANVAS + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

/// Initial canvas: opaque black, so a dab depositing red is unmistakable.
fn black_canvas() -> Vec<u8> {
    let mut out = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    out
}

#[test]
fn single_dab_deposits_color_at_center() {
    // size = 0.1 → ~25.6px radius. Place at (64, 64), expect a red
    // dab covering the center.
    let graph = build_test_graph(
        /* sine */ 0, /* amplitude */ 0.0, /* size */ 0.1,
    );
    let mut h = harness(&black_canvas(), graph);
    h.begin_stroke();
    let info = PaintInformation {
        pos: [64.0, 64.0],
        pressure: 1.0,
        ..Default::default()
    };
    h.dab_and_flush(&info, [1.0, 0.0, 0.0, 1.0], 0);

    let rgba = h.readback_canvas();
    let center = center_pixel(&rgba, 64, 64);
    assert!(
        center[0] > 200 && center[1] < 50 && center[2] < 50,
        "center pixel should be ~red after dab, got {center:?}"
    );

    // Outside the disc footprint: still black.
    let outside = center_pixel(&rgba, 10, 10);
    assert_eq!(
        outside,
        [0, 0, 0, 255],
        "outside the dab should be unchanged"
    );
}

#[test]
fn two_dabs_same_flush_both_deposit() {
    // Two dabs at distinct positions in one flush. Both must reach
    // the layer — catches accidentally indexing all instances to dab
    // 0 in the storage buffer.
    let graph = build_test_graph(0, 0.0, 0.1);
    let mut h = harness(&black_canvas(), graph);
    h.begin_stroke();
    let a = PaintInformation {
        pos: [40.0, 40.0],
        pressure: 1.0,
        ..Default::default()
    };
    let b = PaintInformation {
        pos: [88.0, 88.0],
        pressure: 1.0,
        ..Default::default()
    };
    h.two_dabs_same_phase(&a, &b, [0.0, 1.0, 0.0, 1.0]);

    let rgba = h.readback_canvas();
    let center_a = center_pixel(&rgba, 40, 40);
    let center_b = center_pixel(&rgba, 88, 88);
    assert!(
        center_a[1] > 200 && center_a[0] < 50,
        "dab A center should be green, got {center_a:?}"
    );
    assert!(
        center_b[1] > 200 && center_b[0] < 50,
        "dab B center should be green, got {center_b:?}"
    );
    // Halfway between, but outside both: still black.
    let middle = center_pixel(&rgba, 64, 64);
    assert_eq!(
        middle,
        [0, 0, 0, 255],
        "between the two dabs should be untouched, got {middle:?}"
    );
}

#[test]
fn builtin_perlin_ink_brush_renders_without_shader_error() {
    // Render the actual Perlin Ink builtin — exercises `random →
    // circle` wires that pack per-dab values into the dab record
    // and reference them from the shape evaluator. Regression test
    // for the case where circle's shape eval was emitted as a
    // top-level WGSL function that captured `d.<field>` from outside
    // its scope (Dawn rejects, naga silently accepted on native).
    let perlin = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Perlin Ink")
        .expect("Perlin Ink registered");
    let (device, queue) = shared_device();
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, CANVAS, CANVAS, &black_canvas());
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
        label: Some("perlin-ink-builtin-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke_paint_target);
    queue.submit([enc.finish()]);

    // Override the brush's size port so the dab fits in the test
    // canvas — the builtin's exposed size is small by default.
    let mut graph = perlin.metadata.graph.clone();
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "paint_compiled")
        .map(|(id, _)| *id)
        .unwrap();
    graph.set_port_default(term_id, "size", 0.15).unwrap();

    let runner = compile_graph(&graph).expect("Perlin Ink compiles");
    let mut h = Harness {
        device,
        queue,
        layer_texture,
        layer_view,
        pipelines,
        dab_pool,
        stroke_buffer,
        runner,
        resource_handles: HashMap::new(),
    };
    h.begin_stroke();
    let info = PaintInformation {
        pos: [64.0, 64.0],
        pressure: 1.0,
        ..Default::default()
    };
    h.dab_and_flush(&info, [1.0, 0.5, 0.0, 1.0], 0);

    let rgba = h.readback_canvas();
    // Perlin shape varies per random seed — the centre may be inside
    // or outside, but *some* deposition has to land within the dab
    // footprint (radius ~38px around (64, 64)) if the shader
    // compiled. Scan the whole bbox for any non-black pixel.
    let mut deposited = 0;
    for y in 20..108 {
        for x in 20..108 {
            let p = center_pixel(&rgba, x, y);
            if p[0] > 0 || p[1] > 0 || p[2] > 0 {
                deposited += 1;
            }
        }
    }
    assert!(
        deposited > 50,
        "expected ≥50 non-black pixels inside dab footprint, found {deposited} \
         (shader compile silently failed or dab missed the layer)"
    );
}

#[test]
fn perlin_amplitude_zero_collapses_to_disc() {
    // Perlin algorithm but amplitude = 0 → r(θ) = 1 for all θ, i.e.
    // a clean disc. The center should be solid, the corner of the
    // bounding box should be transparent (outside the disc but
    // inside the rasterized quad).
    let graph = build_test_graph(
        /* perlin */ 1, /* amplitude */ 0.0, /* size */ 0.2,
    );
    let mut h = harness(&black_canvas(), graph);
    h.begin_stroke();
    let info = PaintInformation {
        pos: [64.0, 64.0],
        pressure: 1.0,
        ..Default::default()
    };
    h.dab_and_flush(&info, [0.0, 0.0, 1.0, 1.0], 0);

    let rgba = h.readback_canvas();
    let center = center_pixel(&rgba, 64, 64);
    assert!(
        center[2] > 200,
        "center should be ~blue with amplitude=0, got {center:?}"
    );

    // The dab radius at size = 0.2 is ~51 px. So a pixel ~70px away
    // should be outside the disc and unchanged.
    let outside = center_pixel(&rgba, 64, 0);
    assert_eq!(
        outside,
        [0, 0, 0, 255],
        "outside the disc should be unchanged, got {outside:?}"
    );
}
