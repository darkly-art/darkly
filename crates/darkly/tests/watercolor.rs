//! Watercolor GPU integration tests.
//!
//! Verifies the watercolor terminal's two-pass blend math end-to-end:
//! ensure_canvas_copy → pickup → composite → commit_scratch_blit.
//!
//! Specifically asserts that the centre pixel of a single dab over a
//! uniform-colour canvas equals `mix(canvas, paint, deposit)` at the
//! three landmark deposit values (0, 0.5, 1).
//!
//! Run with `cargo test -p darkly --test watercolor`.
//!
//! Modelled after `tests/liquify.rs` — same shared-device pattern, same
//! StrokeBuffer/PaintTarget setup, same `make_ctx!` macro shape.

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

/// Build a watercolor graph: pen_input.position → watercolor.position,
/// circle (hard edge) → stamp.tip, paint_color (default white) → stamp.color
/// AND watercolor.color, stamp → watercolor.{dab, dab_size, brush_preview}.
/// `size`, `deposit`, and `wetness` are pinned to the test's requested values.
fn watercolor_graph(size: f32, deposit: f32, wetness: f32) -> Graph<BrushWireType> {
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
    let watercolor = graph.add_node(
        "watercolor",
        registry.get("watercolor").unwrap().ports.clone(),
        vec![],
    );

    // Hard edge so the centre pixel has mask = 1 — gives a deterministic
    // assertion target. Soft circles bias the centre slightly because the
    // SDF can falloff before the geometric edge.
    graph.set_port_default(circle, "softness", 0.0).unwrap();
    graph.set_port_default(stamp, "size", size).unwrap();
    graph
        .set_port_default(watercolor, "deposit", deposit)
        .unwrap();
    graph
        .set_port_default(watercolor, "wetness", wetness)
        .unwrap();
    // Per-dab opacity = 1 so we're testing the blend math, not the dimmer.
    graph.set_port_default(watercolor, "opacity", 1.0).unwrap();

    let wires = [
        (circle, "texture", stamp, "tip"),
        (paint_color, "color", stamp, "color"),
        (paint_color, "color", watercolor, "color"),
        (stamp, "dab", watercolor, "dab"),
        (stamp, "dab_size", watercolor, "dab_size"),
        (pen, "position", watercolor, "position"),
        (stamp, "preview", watercolor, "brush_preview"),
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

fn harness(initial: &[u8], size: f32, deposit: f32, wetness: f32) -> Harness {
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
        label: Some("watercolor-test-pre-stroke-init"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke_paint_target);
    queue.submit([enc.finish()]);

    let graph = watercolor_graph(size, deposit, wetness);
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

/// Build the same brush as `harness`, but use an `image` node (with a
/// pre-uploaded resource named "tip") instead of a procedural circle, plus
/// `pen.pressure → stamp.flow`. Mirrors the real builtin watercolor brush
/// graph for end-to-end reproduction tests.
fn harness_image_tip(initial: &[u8], size: f32, deposit: f32, wetness: f32) -> Harness {
    let (device, queue) = shared_device();
    let (layer_texture, layer_view) = create_test_texture(&device, &queue, CANVAS, CANVAS, initial);
    let mut dab_pool = DabTexturePool::new(&device);
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
        label: Some("watercolor-test-pre-stroke-init"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke_paint_target);
    queue.submit([enc.finish()]);

    // Upload an 8×8 fully-white opaque image as the "tip" resource. The
    // image node will look it up by name; the stamp will sample it as a
    // grayscale alpha mask (default AlphaMask application mode).
    let tip_pixels = vec![255u8; 8 * 8 * 4];
    let tip_handle = dab_pool.upload_image(&device, &queue, "tip", 8, 8, &tip_pixels);
    let mut resource_handles = HashMap::new();
    resource_handles.insert("tip".to_string(), tip_handle);

    let graph = watercolor_image_graph(size, deposit, wetness);
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
        resource_handles,
    }
}

/// Image-tip variant of `watercolor_graph`. Mirrors the real builtin: image
/// node feeds stamp.tip, paint_color wires to BOTH stamp.color and
/// watercolor.color, pen.pressure feeds stamp.flow.
fn watercolor_image_graph(size: f32, deposit: f32, wetness: f32) -> Graph<BrushWireType> {
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
    let image = graph.add_node(
        "image",
        registry.get("image").unwrap().ports.clone(),
        vec![ParamValue::String("tip".into())],
    );
    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![],
    );
    let watercolor = graph.add_node(
        "watercolor",
        registry.get("watercolor").unwrap().ports.clone(),
        vec![],
    );

    graph.set_port_default(stamp, "size", size).unwrap();
    graph
        .set_port_default(watercolor, "deposit", deposit)
        .unwrap();
    graph
        .set_port_default(watercolor, "wetness", wetness)
        .unwrap();
    graph.set_port_default(watercolor, "opacity", 1.0).unwrap();

    let wires = [
        (image, "texture", stamp, "tip"),
        (paint_color, "color", stamp, "color"),
        (paint_color, "color", watercolor, "color"),
        (pen, "pressure", stamp, "flow"),
        (stamp, "dab", watercolor, "dab"),
        (stamp, "dab_size", watercolor, "dab_size"),
        (pen, "position", watercolor, "position"),
        (stamp, "preview", watercolor, "brush_preview"),
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
            pending_dabs: Vec::new(),
            pending_dabs_row_range: None,
        }
    }};
}

impl Harness {
    fn begin_stroke(&mut self) {
        // Take ownership of the resources to satisfy disjoint borrows; rebuild
        // before returning. The runner doesn't retain `&resources` across
        // calls — `make_ctx!` borrows it for one encoder lifetime only.
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "watercolor-test-begin", &resources);
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
            let mut ctx = make_ctx!(self, "watercolor-test-dab", &resources);
            self.runner.execute_gpu(&mut ctx);
            self.queue.submit([ctx.encoder.finish()]);
        }
        self.resource_handles = resources;
    }

    fn commit(&mut self) {
        let resources = std::mem::take(&mut self.resource_handles);
        {
            let mut ctx = make_ctx!(self, "watercolor-test-commit", &resources);
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

fn solid_red_canvas() -> Vec<u8> {
    let mut pixels = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for chunk in pixels.chunks_exact_mut(4) {
        chunk[0] = 255;
        chunk[3] = 255;
    }
    pixels
}

/// Build a paint event where `distance` is non-zero so the stroke engine's
/// "first stationary dab" gates don't skip it.
fn pen(pos: [f32; 2]) -> PaintInformation {
    PaintInformation {
        pos,
        distance: 10.0,
        pressure: 1.0,
        ..Default::default()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// `deposit = 1.0`: pure paint deposit. Equivalent to the regular stamp
/// brush — paint colour ends up on the canvas at the brush centre.
/// `deposit = 1` with `wetness = 1` puts paint into the brush load (no
/// canvas in the mix) and stamps it at full alpha — a regular paint stamp.
#[test]
fn deposit_full_paints_paint_color() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.2, 1.0, 1.0); // deposit=1, wetness=1
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue
    h.commit();

    let after = h.readback();
    let center = pixel(&after, 64, 64);
    assert!(
        center[0] < 16 && center[1] < 16 && center[2] > 240,
        "deposit=1.0 over red, paint=blue: centre should be blue, got {:?}",
        center,
    );
}

/// `wetness = 0` is a true no-op regardless of deposit and canvas state —
/// `fg_a = 0` because wetness multiplies straight into the alpha gate.
#[test]
fn noop_when_wetness_zero() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.2, 1.0, 0.0); // deposit=1, wetness=0
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]);
    h.commit();

    let after = h.readback();
    assert_eq!(
        after, initial,
        "wetness=0 must be a no-op regardless of deposit"
    );
}

/// `deposit = 0.0` is a pure flow gate — the brush moves but stamps
/// nothing, regardless of wetness. Layer stays byte-identical.
#[test]
fn deposit_zero_is_noop() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.2, 0.0, 1.0); // deposit=0, wetness=1
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue (ignored)
    h.commit();

    let after = h.readback();
    assert_eq!(
        after, initial,
        "deposit=0 must be a no-op regardless of wetness — got modified pixels"
    );
}

/// `deposit = 0` with `wetness = 1` over a painted canvas pulls the canvas
/// into the brush load and stamps it back — pure smudge. On uniform red
/// the result is still red (smudge of red = red), but the test verifies
/// the paint colour did NOT bleed through.
#[test]
fn full_smudge_stamps_canvas_colour() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.2, 0.0, 1.0); // deposit=0, wetness=1
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue (should NOT show)
    h.commit();

    let after = h.readback();
    let center = pixel(&after, 64, 64);
    assert!(
        center[0] > 240 && center[1] < 16 && center[2] < 16,
        "deposit=0 wetness=1 should stamp canvas (red), not paint (blue); got {:?}",
        center,
    );
}

/// Mid `deposit = 0.5` with `wetness = 1.0` mixes paint and canvas 50/50
/// in the brush load, then stamps that mixed colour at full alpha. With
/// paint=blue and canvas=red, the load is purple `(0.5, 0, 0.5)`, source-
/// over with red gives the same purple at the centre.
#[test]
fn mid_deposit_blends_paint_and_canvas() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.2, 0.5, 1.0); // deposit=0.5, wetness=1
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue
    h.commit();

    let after = h.readback();
    let center = pixel(&after, 64, 64);
    // load_rgb = mix(red, blue, 0.5) = (0.5, 0, 0.5). Stamped at full
    // alpha source-over with red also yields purple. Tolerate ±8 LSB.
    assert!(
        (center[0] as i32 - 127).abs() <= 8,
        "deposit=0.5 R channel: expected ~127, got {}",
        center[0],
    );
    assert!(
        center[1] <= 8,
        "deposit=0.5 G channel: expected ~0, got {}",
        center[1],
    );
    assert!(
        (center[2] as i32 - 127).abs() <= 8,
        "deposit=0.5 B channel: expected ~127, got {}",
        center[2],
    );
}

/// Mid `wetness = 0.5` translucently stamps the brush load. With deposit=1
/// the load is paint; at half wetness, paint goes on at half alpha and
/// blends with the canvas via source-over. Same purple at the centre as
/// the mid-deposit test, but reached via alpha blending instead of the
/// load mix.
#[test]
fn mid_wetness_translucent_stamp() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.2, 1.0, 0.5); // deposit=1, wetness=0.5
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue
    h.commit();

    let after = h.readback();
    let center = pixel(&after, 64, 64);
    // load = blue, fg_a = 0.5, source_over with red bg → (0.5, 0, 0.5).
    assert!((center[0] as i32 - 127).abs() <= 8, "R: {}", center[0]);
    assert!(center[1] <= 8, "G: {}", center[1]);
    assert!((center[2] as i32 - 127).abs() <= 8, "B: {}", center[2]);
}

/// Pixels well outside the brush footprint are byte-identical to the
/// pre-stroke canvas. begin_stroke seeds the scratch from pre_stroke; the
/// composite shader's load+source-over preserves anything outside the dab
/// quad; commit blits the whole scratch back. So the top-left corner of a
/// solid-red canvas should remain solid red after a centre dab.
#[test]
fn outside_brush_footprint_unchanged() {
    let initial = solid_red_canvas();
    let mut h = harness(&initial, 0.05, 1.0, 1.0);
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]);
    h.commit();

    let after = h.readback();
    // (0,0) is far outside a tiny ~12px-radius brush at (64,64).
    assert_eq!(
        pixel(&after, 0, 0),
        [255, 0, 0, 255],
        "corner pixel should stay original red",
    );
    assert_eq!(
        pixel(&after, 127, 127),
        [255, 0, 0, 255],
        "opposite corner should stay original red",
    );
}

/// On a fully transparent canvas, `deposit=0.0` (pure smudge) must be a
/// no-op — there's nothing to smudge with, so the brush should leave the
/// canvas unchanged. The user-reported bug was the brush painting black
/// here (the present-pass clear bleeding through unchanged transparent
/// pixels was masked as "the brush deposited black").
///
/// Two regression checks at deposit=0 over empty canvas:
///   1. center pixel stays transparent (alpha = 0, RGB irrelevant)
///   2. the same brush at deposit=1 still paints — proves deposit gating
///      is the only thing being suppressed, not the brush wholesale
#[test]
fn deposit_zero_on_transparent_canvas_is_noop() {
    let initial = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    // wetness=1 to *enable* the brush; deposit=0 over transparent gives
    // load_alpha = mix(canvas_a=0, paint_a=1, 0) = 0, so fg_a = 0 → no
    // deposit. This is what makes "smudge a blank layer" do nothing
    // instead of painting black or unintended paint.
    let mut h = harness_image_tip(&initial, 0.2, 0.0, 1.0);
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue
    h.commit();

    let after = h.readback();
    let center = pixel(&after, 64, 64);
    // No canvas to smudge → deposit=0 should leave the layer transparent.
    assert!(
        center[3] < 4,
        "deposit=0 on empty canvas must not deposit anything (was the \
         user-reported black-deposit bug); centre alpha = {}, expected ~0",
        center[3],
    );
}

/// Sibling of the no-op test: at `deposit=1.0` on a transparent canvas the
/// brush behaves like a regular stamp — the deposit gate opens and paint
/// flows. Confirms the empty-canvas path still works at the other extreme.
#[test]
fn deposit_full_on_transparent_canvas_paints_paint_color() {
    let initial = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    let mut h = harness_image_tip(&initial, 0.2, 1.0, 1.0); // deposit=1, wetness=1
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue
    h.commit();

    let after = h.readback();
    let center = pixel(&after, 64, 64);
    assert!(
        center[2] > 200 && center[0] < 32 && center[1] < 32,
        "deposit=1 on empty canvas should stamp paint colour, got {:?}",
        center,
    );
}

/// Regression: watercolor on a layer larger than the canvas (paste-extent
/// or off-canvas-grown layer) must not corrupt pixels in the off-canvas
/// strip.
///
/// The bug: `WatercolorEvaluator::begin_stroke` copied a canvas-sized
/// rectangle from pre_stroke into stroke_scratch, but both textures are
/// layer-sized. When the layer extends past the canvas, the strip beyond
/// canvas dims was left uninitialized (transparent black) in the scratch.
/// `commit_scratch_blit` then blits the full layer-sized scratch back to
/// the layer, overwriting the existing pixels in the off-canvas strip with
/// transparent black — manifesting as the rightmost ~1/3 of a wide layer
/// going black after a single watercolor dab.
#[test]
fn off_canvas_strip_preserved_on_oversized_layer() {
    let (device, queue) = shared_device();

    // Canvas is 128×128; layer is 192×128 (50% wider, simulating a
    // paste-extent or off-canvas-grown layer extending past the right
    // edge of the canvas).
    let layer_w = CANVAS + CANVAS / 2;
    let layer_h = CANVAS;

    let mut initial = vec![0u8; (layer_w * layer_h * 4) as usize];
    for chunk in initial.chunks_exact_mut(4) {
        chunk[0] = 255;
        chunk[3] = 255;
    }

    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, layer_w, layer_h, &initial);

    let mut dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout());

    // Stroke buffer is sized to the layer (matches engine/painting.rs:610),
    // not the canvas — that's the precondition that exposes the bug.
    let mut stroke_buffer = StrokeBuffer::new(
        &device,
        layer_w,
        layer_h,
        dab_pool.bind_group_layout(),
        &pipelines,
    );

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("watercolor-oversized-layer-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(
        &device,
        &mut enc,
        &pipelines,
        &darkly::gpu::paint_target::GpuPaintTarget::from_extent(
            &layer_texture,
            &layer_view,
            wgpu::TextureFormat::Rgba8Unorm,
            darkly::coord::CanvasRect::from_xywh(0, 0, layer_w, layer_h),
            CANVAS,
            CANVAS,
        ),
    );
    queue.submit([enc.finish()]);

    let graph = watercolor_graph(0.05, 1.0, 1.0);
    let mut runner = compile_graph(&graph).expect("graph compiles");
    let resources: HashMap<String, TextureHandle> = HashMap::new();

    macro_rules! ctx_for {
        ($label:expr) => {{
            let (scratch, pre_stroke_texture, pre_stroke_bind_group) =
                stroke_buffer.parts_for_brush_ctx();
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
                paint_target: Some(darkly::gpu::paint_target::GpuPaintTarget::from_extent(
                    &layer_texture,
                    &layer_view,
                    wgpu::TextureFormat::Rgba8Unorm,
                    darkly::coord::CanvasRect::from_xywh(0, 0, layer_w, layer_h),
                    CANVAS,
                    CANVAS,
                )),
                selection_bind_group: pipelines.default_selection_bind_group(),
                preview_target_view: None,
                resource_handles: &resources,
                blend_mode: 0,
                preview_mask_view: None,
                preview_mask_size: (0, 0),
                brush_preview_info: None,
                pre_stroke_texture: Some(pre_stroke_texture),
                pre_stroke_bind_group: Some(pre_stroke_bind_group),
                dab_write_canvas_bbox: None,
                perf: BrushPerfCounters::default(),
                pending_dabs: Vec::new(),
                pending_dabs_row_range: None,
            }
        }};
    }

    {
        let mut ctx = ctx_for!("watercolor-oversized-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }

    let info = pen([CANVAS as f32 / 2.0, CANVAS as f32 / 2.0]);
    runner.clear_slots();
    runner.seed_sensors(&info, [0.0, 0.0, 1.0, 1.0], 0, info.index);
    runner.execute_cpu();
    {
        let mut ctx = ctx_for!("watercolor-oversized-dab");
        runner.execute_gpu(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }

    {
        let mut ctx = ctx_for!("watercolor-oversized-commit");
        runner.commit(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }

    let after = readback_texture(
        &device,
        &queue,
        &layer_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        layer_w,
        layer_h,
    );

    // Pixel at (160, 64): well inside the off-canvas strip (x ∈ 128..192),
    // far from any dab. Must remain opaque red — if it's transparent black
    // the bug is present.
    let off_x = CANVAS + CANVAS / 4;
    let off_y = CANVAS / 2;
    let i = ((off_y * layer_w + off_x) * 4) as usize;
    let off_pixel = [after[i], after[i + 1], after[i + 2], after[i + 3]];
    assert_eq!(
        off_pixel,
        [255, 0, 0, 255],
        "Off-canvas pixel at ({off_x}, {off_y}) corrupted by watercolor \
         stroke. The layer's right strip beyond the canvas got overwritten \
         with transparent black — `WatercolorEvaluator::begin_stroke` \
         seeds only a canvas-sized region of the scratch, leaving the rest \
         uninitialized; commit_scratch_blit then blits the full \
         layer-sized scratch back over the layer."
    );
}

/// Reproduces the bug "watercolor brush deposits black always". Mirrors the
/// real builtin watercolor brush graph (image-based tip, pen.pressure→flow,
/// paint_color wired to BOTH stamp.color AND watercolor.color) on a
/// transparent canvas. With paint = blue and deposit = 1.0, the centre
/// pixel must be blue — if it's black, the wire from paint_color to
/// watercolor.color isn't propagating.
#[test]
fn image_tip_paints_paint_color_at_full_deposit() {
    // Transparent canvas — matches what a user typically paints on. Note:
    // pre_stroke.rgb = 0 here, which is exactly the "always black"
    // scenario the user reported.
    let initial = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    let mut h = harness_image_tip(&initial, 0.2, 1.0, 1.0);
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0]), [0.0, 0.0, 1.0, 1.0]); // paint = blue
    h.commit();

    let after = h.readback();
    let center = pixel(&after, 64, 64);
    assert!(
        center[2] > 200 && center[0] < 32,
        "image-tip + paint_color → watercolor.color must propagate: \
         expected blue at centre, got {:?}",
        center,
    );
}
