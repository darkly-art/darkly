//! Circle node algorithm + centroid alignment tests.
//!
//! Renders the circle dab texture for each algorithm, reads it back, and
//! asserts on:
//!
//! - **Back-compat**: `algorithm=Sine, amplitude=0` matches the unmodulated
//!   disc visually (centre filled, corner empty, edge near the original
//!   ~0.498 viewport-radius mark).
//! - **Per-algorithm coverage**: each algorithm produces non-zero coverage at
//!   the texture centre and zero coverage well outside `r_max`.
//! - **Centroid alignment**: for asymmetric configs (sine n=1 kidney,
//!   superformula m=1, lopsided Perlin), the rendered alpha mask's centroid
//!   sits within ±2 px of the texture centre. This is the load-bearing
//!   regression test for the CPU-side centroid integration: any drift
//!   between the Rust `r_theta` and the WGSL `r_theta` shows up here.
//!
//! Run with `cargo test -p darkly --test circle_node`.
//!
//! Modelled after `tests/watercolor.rs` — same shared-device pattern, same
//! direct-evaluator pattern but at the leaf level (no graph, just call
//! `CircleEvaluator::evaluate_gpu` against a hand-built `EvalContext`).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use darkly::brush::dab_pool::{DabTexturePool, MAX_DAB_SIZE};
use darkly::brush::eval::{BrushNodeEvaluator, EvalContext};
use darkly::brush::gpu_context::BrushGpuContext;
use darkly::brush::nodes::circle::CircleEvaluator;
use darkly::brush::pipelines::BrushPipelines;
use darkly::brush::wire::{BrushWireType, ScalarValue};
use darkly::brush::BrushNodeRegistry;
use darkly::gpu::params::ParamValue;
use darkly::gpu::test_utils::{readback_texture, test_device};
use darkly::nodegraph::{NodeId, PortDef};

const DAB: u32 = MAX_DAB_SIZE;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

/// All circle ports we care about, with their values for one render.
struct Inputs {
    softness: f32,
    amplitude: f32,
    frequency: f32,
    phase: f32,
    persistence: f32,
    seed: f32,
    octaves: f32,
    n1: f32,
    n2: f32,
    n3: f32,
}

impl Default for Inputs {
    fn default() -> Self {
        // Matches the registration defaults: hard edge, unmodulated disc.
        Self {
            softness: 0.0,
            amplitude: 0.0,
            frequency: 6.0,
            phase: 0.0,
            persistence: 0.5,
            seed: 0.0,
            octaves: 3.0,
            n1: 1.0,
            n2: 1.0,
            n3: 1.0,
        }
    }
}

/// Render one circle dab and read back the RGBA8 pixels.
///
/// `algorithm`: 0 = sine, 1 = perlin, 2 = superformula. Read directly from
/// the registered enum in `register()`.
fn render_dab(algorithm: i32, inputs: Inputs) -> Vec<u8> {
    let (device, queue) = shared_device();
    let mut dab_pool = DabTexturePool::new(&device);
    let pipelines = BrushPipelines::new(&device, &queue, dab_pool.bind_group_layout(), DAB, DAB);

    // Pull the circle node's port_defs from the registry — keeps the test
    // honest if registration changes.
    let registry = BrushNodeRegistry::new();
    let reg = registry.get("circle").expect("circle registered");
    let port_defs: Vec<PortDef<BrushWireType>> = reg.ports.clone();

    // EvalContext requires a `&HashMap<String, ScalarValue>` of resolved
    // input values. For ports with no upstream wire, the runtime would fall
    // back to port defaults; here we override every value the test cares
    // about explicitly so the assertion is anchored to known inputs.
    let mut input_map: HashMap<String, ScalarValue> = HashMap::new();
    input_map.insert("softness".into(), ScalarValue::Scalar(inputs.softness));
    input_map.insert("amplitude".into(), ScalarValue::Scalar(inputs.amplitude));
    input_map.insert("frequency".into(), ScalarValue::Scalar(inputs.frequency));
    input_map.insert("phase".into(), ScalarValue::Scalar(inputs.phase));
    input_map.insert(
        "persistence".into(),
        ScalarValue::Scalar(inputs.persistence),
    );
    input_map.insert("seed".into(), ScalarValue::Scalar(inputs.seed));
    input_map.insert("octaves".into(), ScalarValue::Scalar(inputs.octaves));
    input_map.insert("n1".into(), ScalarValue::Scalar(inputs.n1));
    input_map.insert("n2".into(), ScalarValue::Scalar(inputs.n2));
    input_map.insert("n3".into(), ScalarValue::Scalar(inputs.n3));

    let params = vec![ParamValue::Int(algorithm)];
    let resources = HashMap::new();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("circle-test-encoder"),
    });
    let view = pipelines
        .canvas_copy_texture()
        .create_view(&Default::default());
    let stroke_view = view; // borrow placeholder; circle doesn't read it.

    // Build a context. The circle evaluator only touches `dab_pool`,
    // `pipelines`, `device`, `queue`, and `encoder` — the other fields are
    // unused for a non-terminal mask node, but the struct requires them.
    let dab_handle;
    {
        let mut ctx = BrushGpuContext {
            encoder,
            device: &device,
            queue: &queue,
            dab_pool: &mut dab_pool,
            pipelines: &pipelines,
            stroke_scratch_view: &stroke_view,
            stroke_scratch_texture: pipelines.canvas_copy_texture(),
            canvas_width: DAB,
            canvas_height: DAB,
            paint_target: None,
            selection_bind_group: pipelines.default_selection_bind_group(),
            resource_handles: &resources,
            blend_mode: 0,
            canvas_copy_origin: None,
            preview_mask_view: None,
            preview_mask_size: (0, 0),
            brush_preview_info: None,
            pre_stroke_texture: None,
            pre_stroke_bind_group: None,
            scratch_bind_group: None,
            dab_write_canvas_bbox: None,
        };
        let eval_ctx = EvalContext {
            inputs: &input_map,
            params: &params,
            port_defs: &port_defs,
            lut: None,
            stroke_seed: 0,
            dab_index: 0,
            node_id: NodeId(1),
        };
        let outputs = CircleEvaluator.evaluate_gpu(&eval_ctx, &mut ctx);
        // Pull the texture handle out of the evaluator's outputs.
        dab_handle = outputs
            .into_iter()
            .find_map(|(name, val)| {
                if name == "texture" {
                    if let ScalarValue::Texture(h) = val {
                        return Some(h);
                    }
                }
                None
            })
            .expect("circle evaluator produced a texture output");
        encoder = ctx.encoder;
    }
    queue.submit([encoder.finish()]);

    let dab_texture = dab_pool.texture(dab_handle);
    readback_texture(
        &device,
        &queue,
        dab_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        DAB,
        DAB,
    )
}

fn alpha_at(pixels: &[u8], x: u32, y: u32) -> u8 {
    let i = ((y * DAB + x) * 4 + 3) as usize;
    pixels[i]
}

/// Compute the alpha-weighted centroid of a rendered dab in pixel
/// coordinates. Returns `(cx_px, cy_px)`. With a properly centroid-aligned
/// shape, both should be ≈ `DAB / 2`.
fn alpha_centroid(pixels: &[u8]) -> (f32, f32) {
    let mut sum = 0.0_f64;
    let mut sx = 0.0_f64;
    let mut sy = 0.0_f64;
    for y in 0..DAB {
        for x in 0..DAB {
            let a = alpha_at(pixels, x, y) as f64;
            sum += a;
            sx += x as f64 * a;
            sy += y as f64 * a;
        }
    }
    if sum < 1.0 {
        return (DAB as f32 * 0.5, DAB as f32 * 0.5);
    }
    ((sx / sum) as f32, (sy / sum) as f32)
}

const ALGO_SINE: i32 = 0;
const ALGO_PERLIN: i32 = 1;
const ALGO_SUPERFORMULA: i32 = 2;

// ── Tests ───────────────────────────────────────────────────────────────────

/// Back-compat regression: with `algorithm=Sine, amplitude=0`, the rendered
/// shape must look like the original hard disc — centre fully covered, far
/// corners empty, and the edge sits at roughly the original 0.498
/// viewport-radius mark. Not asserted byte-identically because the new
/// shader has slightly different sub-pixel softness math (see circle.wgsl).
#[test]
fn back_compat_unmodulated_disc() {
    let pixels = render_dab(
        ALGO_SINE,
        Inputs {
            amplitude: 0.0,
            ..Default::default()
        },
    );

    // Centre fully covered.
    assert_eq!(
        alpha_at(&pixels, DAB / 2, DAB / 2),
        255,
        "centre pixel should be fully opaque",
    );

    // Far corner fully transparent.
    assert_eq!(
        alpha_at(&pixels, 0, 0),
        0,
        "far corner should be fully transparent",
    );

    // The edge along the +x axis sits at ~0.498 of the viewport — pixel
    // ~(0.5 + 0.498) * DAB ≈ 0.998 * DAB. So pixel at 0.95*DAB should still
    // be opaque (well inside) and pixel at DAB-1 should be transparent
    // (just outside the AA band).
    let inside = alpha_at(&pixels, (DAB as f32 * 0.95) as u32, DAB / 2);
    assert!(
        inside > 200,
        "0.95*DAB along +x should still be inside the disc, got alpha {inside}",
    );
    let outside = alpha_at(&pixels, DAB - 1, DAB / 2);
    assert_eq!(
        outside, 0,
        "DAB-1 along +x should be outside the disc and the AA band",
    );
}

/// Each algorithm produces a non-empty mask with centre coverage. Smoke test
/// that the algorithm enum dispatches correctly and no formula returns
/// degenerate (all-zero) output for typical parameters.
#[test]
fn each_algorithm_produces_centre_coverage() {
    for (algo, inputs, name) in [
        (
            ALGO_SINE,
            Inputs {
                amplitude: 0.2,
                frequency: 6.0,
                ..Default::default()
            },
            "sine",
        ),
        (
            ALGO_PERLIN,
            Inputs {
                amplitude: 0.3,
                frequency: 4.0,
                seed: 42.0,
                octaves: 3.0,
                ..Default::default()
            },
            "perlin",
        ),
        (
            ALGO_SUPERFORMULA,
            Inputs {
                frequency: 5.0,
                n1: 1.0,
                n2: 1.0,
                n3: 1.0,
                ..Default::default()
            },
            "superformula",
        ),
    ] {
        let pixels = render_dab(algo, inputs);
        assert_eq!(
            alpha_at(&pixels, DAB / 2, DAB / 2),
            255,
            "{name}: centre pixel should be fully opaque",
        );
        // Sanity: there's actually a shape, not a single pixel.
        let neighbour = alpha_at(&pixels, DAB / 2 + 4, DAB / 2);
        assert!(
            neighbour > 200,
            "{name}: pixels near centre should also be covered, got alpha {neighbour}",
        );
    }
}

/// Centroid alignment regression — the load-bearing test for centroid
/// correction. For three asymmetric shape configurations (chosen because
/// their natural-coord centroid is provably non-zero), the *rendered* alpha
/// mask's centroid must land at the texture centre. Tolerance is ±2 px to
/// allow for sub-pixel AA bias in the centroid integration of the discrete
/// alpha mask itself; the real bug we're guarding against (formula drift,
/// sign error) would shift the centroid by tens of pixels.
#[test]
fn centroid_lands_at_texture_centre_for_asymmetric_shapes() {
    let centre = DAB as f32 * 0.5;
    let tol = 2.0_f32;

    let cases: &[(i32, &str, Inputs)] = &[
        (
            ALGO_SINE,
            "sine n=1 kidney",
            Inputs {
                amplitude: 0.3,
                frequency: 1.0, // single bump = max asymmetry
                phase: 0.0,
                ..Default::default()
            },
        ),
        (
            ALGO_SUPERFORMULA,
            "superformula m=1 lopsided",
            Inputs {
                frequency: 1.0,
                n1: 1.0,
                n2: 1.0,
                n3: 2.0,
                ..Default::default()
            },
        ),
        (
            ALGO_PERLIN,
            "perlin lopsided seed",
            Inputs {
                amplitude: 0.4,
                frequency: 3.0,
                seed: 7.0, // any seed produces an asymmetric blob
                octaves: 2.0,
                ..Default::default()
            },
        ),
    ];

    for (algo, name, inputs) in cases {
        let pixels = render_dab(*algo, Inputs { ..*inputs });
        let (cx, cy) = alpha_centroid(&pixels);
        assert!(
            (cx - centre).abs() < tol && (cy - centre).abs() < tol,
            "{name}: rendered centroid ({cx}, {cy}) should be within ±{tol} of \
             texture centre ({centre}, {centre}) — drift indicates the Rust \
             centroid integration is out of sync with the WGSL r(θ) formula",
        );
    }
}
