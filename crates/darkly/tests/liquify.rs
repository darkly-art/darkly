//! Liquify GPU integration tests.
//!
//! Exercises the full `begin_stroke` → per-dab `evaluate_gpu` → `commit`
//! lifecycle and verifies:
//! - Pixels inside the disc displace along the motion vector.
//! - Zero motion (or disabled dabs) leaves the canvas unchanged.
//! - Pixels outside the disc are untouched.
//! - Softness waveshape actually differs between saw / sine / square.
//! - `begin_stroke` is idempotent: replaying a sequence after a full rewind
//!   produces the same final canvas (no warp compounding across rewinds).
//!
//! Run with `cargo test -p darkly --test liquify`.

use std::collections::HashMap;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::BrushGpuContext;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipelines::BrushPipelines;
use darkly::brush::dab_pool::DabTexturePool;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::brush::wire::BrushWireType;
use darkly::brush::BrushNodeRegistry;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};
use darkly::nodegraph::{Graph, PortRef};

const CANVAS: u32 = 128;

// ── Test harness ────────────────────────────────────────────────────────────

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layer_texture: wgpu::Texture,
    layer_view: wgpu::TextureView,
    pipelines: BrushPipelines,
    dab_pool: DabTexturePool,
    stroke_buffer: StrokeBuffer,
    runner: BrushGraphRunner,
}

/// Build a minimal liquify graph: pen_input.position/motion → liquify.*,
/// with size/strength/softness overridden to the test's requested values.
fn liquify_graph(
    size: f32,
    strength: f32,
    softness: f32,
) -> Graph<BrushWireType> {
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let liquify = graph.add_node(
        "liquify",
        registry.get("liquify").unwrap().ports.clone(),
        vec![],
    );

    graph.set_port_default(liquify, "size", size).unwrap();
    graph.set_port_default(liquify, "strength", strength).unwrap();
    graph.set_port_default(liquify, "softness", softness).unwrap();

    graph.connect(
        PortRef { node: pen, port: "position".into() },
        PortRef { node: liquify, port: "position".into() },
    ).unwrap();
    graph.connect(
        PortRef { node: pen, port: "motion".into() },
        PortRef { node: liquify, port: "motion".into() },
    ).unwrap();

    graph
}

fn harness(initial: &[u8], size: f32, strength: f32, softness: f32) -> Harness {
    let (device, queue) = test_device();

    let (layer_texture, layer_view) = create_test_texture(&device, &queue, CANVAS, CANVAS, initial);

    let dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout(), CANVAS, CANVAS);

    let stroke_buffer = StrokeBuffer::new(
        &device, CANVAS, CANVAS,
        dab_pool.bind_group_layout(),
        pipelines.canvas_copy_bind_group_layout(),
    );

    // Snapshot the (untouched) layer into pre_stroke, same as the real engine
    // does at the start of a stroke. begin_stroke will copy this into the
    // scratch.
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test-pre-stroke-init"),
    });
    stroke_buffer.save_pre_stroke(&mut enc, &layer_texture);
    queue.submit([enc.finish()]);

    let graph = liquify_graph(size, strength, softness);
    let runner = compile_graph(&graph).expect("graph compiles");

    Harness {
        device, queue, layer_texture, layer_view, pipelines, dab_pool,
        stroke_buffer, runner,
    }
}

/// Build a `BrushGpuContext` inline at the call site. The context borrows
/// individual fields of the harness rather than `&mut self` as a whole, so
/// we can still call `self.runner.*` afterwards (which borrows a disjoint
/// field). Mirrors the engine's `make_gpu_ctx!` pattern in `painting.rs`.
macro_rules! make_ctx {
    ($h:ident, $label:expr, $resources:expr) => {
        BrushGpuContext {
            encoder: $h.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some($label),
            }),
            device: &$h.device,
            queue: &$h.queue,
            dab_pool: &mut $h.dab_pool,
            pipelines: &$h.pipelines,
            stroke_scratch_view: $h.stroke_buffer.stroke_view(),
            stroke_scratch_texture: $h.stroke_buffer.stroke_texture(),
            canvas_width: CANVAS,
            canvas_height: CANVAS,
            selection_bind_group: $h.pipelines.default_selection_bind_group(),
            resource_handles: $resources,
            blend_mode: 0,
            canvas_copy_origin: None,
            preview_mask_view: None,
            preview_mask_size: (0, 0),
            brush_preview_info: None,
            layer_view: Some(&$h.layer_view),
            layer_texture: Some(&$h.layer_texture),
            pre_stroke_texture: Some($h.stroke_buffer.pre_stroke_texture()),
            pre_stroke_bind_group: Some($h.stroke_buffer.pre_stroke_bind_group()),
            scratch_bind_group: Some($h.stroke_buffer.stroke_bind_group()),
            dab_write_bbox: None,
        }
    };
}

impl Harness {
    /// Run `begin_stroke` on the runner and submit.
    fn begin_stroke(&mut self) {
        let resources = HashMap::new();
        let mut ctx = make_ctx!(self, "liquify-test-begin", &resources);
        self.runner.begin_stroke(&mut ctx);
        self.queue.submit([ctx.encoder.finish()]);
    }

    /// Evaluate a single dab and submit.
    fn dab(&mut self, info: &PaintInformation) {
        let resources = HashMap::new();
        // Slot updates borrow the runner mutably but don't need the ctx, so
        // split them out first.
        self.runner.clear_slots();
        self.runner.seed_sensors(info, [1.0, 1.0, 1.0, 1.0], 0, info.index);
        self.runner.execute_cpu();

        let mut ctx = make_ctx!(self, "liquify-test-dab", &resources);
        self.runner.execute_gpu(&mut ctx);
        self.queue.submit([ctx.encoder.finish()]);
    }

    /// Run `commit` on the runner and submit — push the scratch onto the layer.
    fn commit(&mut self) {
        let resources = HashMap::new();
        let mut ctx = make_ctx!(self, "liquify-test-commit", &resources);
        self.runner.commit(&mut ctx);
        self.queue.submit([ctx.encoder.finish()]);
    }

    fn readback(&self) -> Vec<u8> {
        readback_texture(
            &self.device, &self.queue, &self.layer_texture,
            wgpu::TextureFormat::Rgba8Unorm, CANVAS, CANVAS,
        )
    }
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * CANVAS + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// A canvas with a 2-column vertical red bar at x=63..=64.
fn canvas_with_bar() -> Vec<u8> {
    let mut pixels = vec![0u8; (CANVAS * CANVAS * 4) as usize];
    for y in 0..CANVAS {
        for &x in &[63u32, 64u32] {
            let i = ((y * CANVAS + x) * 4) as usize;
            pixels[i] = 255;
            pixels[i + 3] = 255;
        }
    }
    pixels
}

fn pen(pos: [f32; 2], motion: [f32; 2]) -> PaintInformation {
    PaintInformation { pos, motion, ..Default::default() }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Rightward motion should push the bar's red pixels to the right.
///
/// With `motion = (20, 0)` and `strength = 1` and `softness = 1` (square,
/// flat inside the disc), the shader samples from `canvas_pos - (20, 0)`.
/// The original bar at x=63..=64 therefore shows up at x=83..=84 inside the
/// disc; the original location shows the background (pre-stroke empty).
#[test]
fn rightward_motion_pushes_pixels_right() {
    let mut h = harness(&canvas_with_bar(), 0.5, 1.0, 1.0);
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0], [20.0, 0.0]));
    h.commit();

    let after = h.readback();

    // Disc of radius 128 (size=0.5 → 0.5 * 512 * 0.5 = 128 px) covers the
    // whole 128×128 canvas; square falloff gives full strength everywhere.
    let shifted = pixel(&after, 83, 64);
    assert!(
        shifted[0] > 200 && shifted[3] > 200,
        "expected red at shifted position (83,64), got {:?}", shifted,
    );
    // The original bar location now samples from (43,64) which was
    // background — expect transparent or empty.
    let orig = pixel(&after, 63, 64);
    assert!(
        orig[0] < 20,
        "expected background at original bar (63,64), got {:?}", orig,
    );
}

/// A dab with zero motion does nothing — the liquify evaluator early-outs
/// before touching the scratch, and commit copies an unmodified scratch-
/// equals-pre_stroke over the layer (identity).
#[test]
fn zero_motion_is_noop() {
    let initial = canvas_with_bar();
    let mut h = harness(&initial, 0.5, 1.0, 1.0);
    h.begin_stroke();
    h.dab(&pen([64.0, 64.0], [0.0, 0.0]));
    h.commit();

    let after = h.readback();
    assert_eq!(
        after, initial,
        "zero-motion dab should leave the canvas byte-identical",
    );
}

/// Pixels far outside the brush disc are untouched.
///
/// A small brush (size=0.05 → radius ~12 px) at (32, 32) cannot reach the
/// bar at x=63. The bar pixels should be byte-identical to pre-stroke.
#[test]
fn outside_radius_is_untouched() {
    let initial = canvas_with_bar();
    let mut h = harness(&initial, 0.05, 1.0, 1.0);
    h.begin_stroke();
    h.dab(&pen([32.0, 32.0], [5.0, 0.0]));
    h.commit();

    let after = h.readback();
    for y in 0..CANVAS {
        // Bar pixels (x=63,64) are ~31 px away from the brush center, well
        // outside the disc.
        assert_eq!(pixel(&after, 63, y), [255, 0, 0, 255], "bar pixel (63,{y})");
        assert_eq!(pixel(&after, 64, y), [255, 0, 0, 255], "bar pixel (64,{y})");
    }
}

/// Saw and square falloff produce meaningfully different outputs.
///
/// Near the edge of the disc the saw waveshape tapers to ~0 displacement,
/// while the square waveshape gives full displacement everywhere inside.
/// The disc edge pixels should therefore differ between the two runs.
#[test]
fn waveshape_differs_saw_vs_square() {
    let initial = canvas_with_bar();

    let mut saw = harness(&initial, 0.25, 1.0, 0.0);  // softness=0 → saw
    saw.begin_stroke();
    saw.dab(&pen([64.0, 64.0], [15.0, 0.0]));
    saw.commit();
    let saw_out = saw.readback();

    let mut square = harness(&initial, 0.25, 1.0, 1.0);  // softness=1 → square
    square.begin_stroke();
    square.dab(&pen([64.0, 64.0], [15.0, 0.0]));
    square.commit();
    let square_out = square.readback();

    // Count pixels that differ — a meaningful (>rounding) delta proves the
    // waveshape math actually affects output.
    let different: usize = saw_out
        .chunks_exact(4)
        .zip(square_out.chunks_exact(4))
        .filter(|(a, b)| {
            a.iter().zip(b.iter()).any(|(x, y)| (*x as i32 - *y as i32).abs() > 4)
        })
        .count();
    assert!(
        different > 100,
        "saw and square waveshapes should produce visibly different outputs, got {different} differing pixels",
    );
}

/// `begin_stroke` is idempotent: replaying a dab sequence with a full rewind
/// midway produces the same final canvas as a single pass. This proves
/// liquify reseeds from the immutable `pre_stroke` snapshot (not from the
/// current layer, which would compound warps exponentially).
#[test]
fn rewind_equivalence() {
    let initial = canvas_with_bar();

    // Run A: three dabs, one pass.
    let mut a = harness(&initial, 0.5, 0.5, 0.5);
    a.begin_stroke();
    a.dab(&pen([40.0, 64.0], [8.0, 0.0]));
    a.dab(&pen([64.0, 64.0], [8.0, 0.0]));
    a.dab(&pen([88.0, 64.0], [8.0, 0.0]));
    a.commit();
    let a_out = a.readback();

    // Run B: two dabs, then a simulated full rewind (begin_stroke + all
    // three dabs replayed) — identical to what the engine does when the
    // stabilizer can't find a checkpoint before a divergence.
    let mut b = harness(&initial, 0.5, 0.5, 0.5);
    b.begin_stroke();
    b.dab(&pen([40.0, 64.0], [8.0, 0.0]));
    b.dab(&pen([64.0, 64.0], [8.0, 0.0]));
    // Rewind: begin_stroke again (reseed scratch from pre_stroke), then
    // replay ALL dabs from vi=0. The commit uses the final scratch state.
    b.begin_stroke();
    b.dab(&pen([40.0, 64.0], [8.0, 0.0]));
    b.dab(&pen([64.0, 64.0], [8.0, 0.0]));
    b.dab(&pen([88.0, 64.0], [8.0, 0.0]));
    b.commit();
    let b_out = b.readback();

    assert_eq!(
        a_out, b_out,
        "rewind+replay must produce identical pixels to a single pass — \
         if this fails, begin_stroke isn't reseeding from pre_stroke \
         (warps are compounding across rewinds)",
    );
}
