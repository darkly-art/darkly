//! Smudge GPU integration tests.
//!
//! Verifies the smudge terminal smears canvas pixels along the stroke and
//! is a true no-op when the dab is stationary (`motion == [0, 0]`). Same
//! shared-device harness shape as `tests/liquify.rs` and `tests/watercolor.rs`.
//!
//! Run with `cargo test -p darkly --test smudge -- --test-threads=1`.

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

// ── Test harness ────────────────────────────────────────────────────────────

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

/// Build a minimal smudge graph: pen_input.position → smudge.position,
/// pen_input.motion → smudge.motion, hard-edged circle → stamp.tip,
/// paint_color → stamp.color, stamp → smudge.{dab,dab_size,brush_preview}.
/// `size` and `rate` are pinned to the test's requested values.
fn smudge_graph(size: f32, rate: f32) -> Graph<BrushWireType> {
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
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![],
    );
    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![],
    );
    let smudge = graph.add_node(
        "smudge",
        registry.get("smudge").unwrap().ports.clone(),
        vec![],
    );

    // Hard edge so the centre pixel has mask = 1 — soft falloff would
    // attenuate the smear at the centre and obscure the assertion.
    graph.set_port_default(circle, "softness", 0.0).unwrap();
    graph.set_port_default(stamp, "size", size).unwrap();
    graph.set_port_default(smudge, "rate", rate).unwrap();
    graph.set_port_default(smudge, "opacity", 1.0).unwrap();

    let wires = [
        (circle, "texture", stamp, "tip"),
        (paint_color, "color", stamp, "color"),
        (stamp, "dab", smudge, "dab"),
        (stamp, "dab_size", smudge, "dab_size"),
        (pen, "position", smudge, "position"),
        (pen, "motion", smudge, "motion"),
        (stamp, "preview", smudge, "brush_preview"),
    ];
    for (from_node, from_port, to_node, to_port) in wires {
        graph
            .connect(
                PortRef {
                    node: from_node,
                    port: from_port.into(),
                },
                PortRef {
                    node: to_node,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }

    graph
}

fn harness(initial: &[u8], size: f32, rate: f32) -> Harness {
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
        label: Some("smudge-test-pre-stroke-init"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke_paint_target);
    queue.submit([enc.finish()]);

    let graph = smudge_graph(size, rate);
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
            pending_compute_dab_bytes: Vec::new(),
            pending_compute_dab_count: 0,
            pending_dabs_row_range: None,
        }
    }};
}

impl Harness {
    fn begin_stroke(&mut self) {
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "smudge-test-begin", &resources);
            self.runner.begin_stroke(&mut ctx);
            self.queue.submit([ctx.encoder.finish()]);
        }
        self.resource_handles = resources;
    }

    fn dab(&mut self, info: &PaintInformation, paint: [f32; 4]) {
        self.runner.clear_slots();
        self.runner.seed_sensors(info, paint, 0, info.index);
        self.runner.execute_cpu();
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "smudge-test-dab", &resources);
            self.runner.execute_gpu(&mut ctx);
            self.queue.submit([ctx.encoder.finish()]);
        }
        self.resource_handles = resources;
    }

    fn commit(&mut self) {
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "smudge-test-commit", &resources);
            self.runner.commit(&mut ctx);
            self.queue.submit([ctx.encoder.finish()]);
        }
        self.resource_handles = resources;
    }

    fn readback(&self) -> Vec<u8> {
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

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * CANVAS + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Layer painted half-red (left of midline) and half-blue (right of
/// midline), both fully opaque. Midline is at `x = CANVAS / 2`.
fn red_blue_split() -> Vec<u8> {
    let mut pixels = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    let midline = CANVAS / 2;
    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let i = ((y * CANVAS + x) * 4) as usize;
            if x < midline {
                pixels[i] = 255; // R
            } else {
                pixels[i + 2] = 255; // B
            }
            pixels[i + 3] = 255; // A
        }
    }
    pixels
}

fn solid_red_canvas() -> Vec<u8> {
    let mut pixels = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for chunk in pixels.chunks_exact_mut(4) {
        chunk[0] = 255;
        chunk[3] = 255;
    }
    pixels
}

/// PaintInformation with a fixed-magnitude motion vector. Distance is
/// non-zero so any future stroke-engine-style "first dab" gates don't
/// skip the dab.
fn pen(pos: [f32; 2], motion: [f32; 2], index: u32) -> PaintInformation {
    PaintInformation {
        pos,
        motion,
        distance: 10.0 + index as f32 * 6.0,
        pressure: 1.0,
        index,
        ..Default::default()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Feature test: stroking the smudge brush from inside the red zone into
/// the blue zone drags red pixels along the stroke direction.
///
/// Asserts, in order of discrimination:
///   1. Midway through the blue zone the sampled pixel is a red-blue mix.
///   2. The red-zone start pixel is unchanged.
///   3. The bleed trails the stroke direction — red(near midline) > red(far).
///      A dulling/averaging smudge implementation would deposit ~equal
///      red across both samples; only true offset-sampled smearing
///      produces a trailing gradient.
#[test]
fn smearing_drags_color_along_stroke() {
    let initial = red_blue_split();
    // size 0.05 → diameter ~25.6 px, radius ~12.8 px. Small enough that
    // the brush footprint near the start doesn't touch the midline.
    let mut h = harness(&initial, 0.05, 0.6);
    h.begin_stroke();

    // March left-to-right across the midline, fixed y = CANVAS / 2.
    // 8 px per dab (motion = [8, 0]). Start at x = 24 (red zone, far from
    // midline by 24+r=36 i.e. >12) and end past the midline well into blue.
    let y = (CANVAS / 2) as f32;
    let step = 8.0_f32;
    let start_x = 24.0_f32;
    let end_x = 104.0_f32; // covers ~80 px / 8 px = 10 dabs
    let mut x = start_x;
    let mut index = 0u32;
    let mut prev: Option<[f32; 2]> = None;
    while x <= end_x + 0.5 {
        let motion = match prev {
            Some([px, py]) => [x - px, y - py],
            None => [0.0, 0.0],
        };
        h.dab(&pen([x, y], motion, index), [0.0, 0.0, 0.0, 1.0]);
        prev = Some([x, y]);
        index += 1;
        x += step;
    }
    h.commit();

    let after = h.readback();

    // 1. Midway through the blue zone, the pixel has been smeared with red.
    //    Pick (88, 64) — well inside the blue zone (x=88 > midline=64), inside
    //    the stroke path.
    let mix_x = 88;
    let mix_y = CANVAS / 2;
    let mid = pixel(&after, mix_x, mix_y);
    assert!(
        mid[0] > 8,
        "blue-zone pixel at ({mix_x}, {mix_y}) should have red from smudge: \
         got R={} G={} B={} A={}",
        mid[0],
        mid[1],
        mid[2],
        mid[3],
    );
    assert!(
        mid[2] > 8,
        "blue-zone pixel at ({mix_x}, {mix_y}) should still have blue: \
         got R={} G={} B={} A={}",
        mid[0],
        mid[1],
        mid[2],
        mid[3],
    );

    // 2. Red-zone start pixel (far outside any brush footprint) is unchanged.
    let pristine = pixel(&after, 4, CANVAS / 2);
    assert_eq!(
        pristine,
        [255, 0, 0, 255],
        "red-zone start pixel must be unchanged",
    );

    // 3. Trailing gradient: red is heavier closer to the midline than far
    //    along the stroke. (72, 64) is just past the midline; (100, 64) is
    //    deeper into the blue zone, late in the stroke.
    let near = pixel(&after, 72, CANVAS / 2);
    let far = pixel(&after, 100, CANVAS / 2);
    assert!(
        near[0] > far[0],
        "smear must trail the stroke direction (red fades along the path): \
         red(near = (72, 64)) = {}, red(far = (100, 64)) = {}",
        near[0],
        far[0],
    );
}

/// Regression: a single stationary dab (first dab of a stroke, `motion =
/// [0, 0]`) must not modify the canvas. The smudge node's stationary-dab
/// early-out short-circuits before the GPU pass; without it the shader
/// would still produce identity output (`mix(bg, bg, _) == bg`), but the
/// early-out is part of the contract.
#[test]
fn stationary_click_does_not_clobber_canvas() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.1, 0.6);
    h.begin_stroke();
    // `motion = [0, 0]` — the `prev == None` path. Pen-down click,
    // no movement yet.
    h.dab(&pen([64.0, 64.0], [0.0, 0.0], 0), [0.0, 0.0, 0.0, 1.0]);
    h.commit();

    let after = h.readback();
    assert_eq!(
        after, initial,
        "single stationary dab (motion=0) must not modify the canvas",
    );
}

/// Regression: two events at the same position must also leave the
/// canvas untouched. Exercises the `prev=Some(pos), curr=pos, dx=dy=0`
/// path independently of `prev=None`. Defends against a bug where the
/// dab-emission chain resurrects motion from a stale field on the
/// second event.
#[test]
fn stationary_two_event_stroke_does_not_clobber_canvas() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.1, 0.6);
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0], [0.0, 0.0], 0), [0.0, 0.0, 0.0, 1.0]);
    h.dab(&pen([64.0, 64.0], [0.0, 0.0], 1), [0.0, 0.0, 0.0, 1.0]);
    h.commit();

    let after = h.readback();
    assert_eq!(
        after, initial,
        "two stationary dabs at the same position must not modify the canvas",
    );
}
