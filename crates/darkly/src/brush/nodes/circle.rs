//! Procedural shape mask GPU node.
//!
//! Renders a closed `r(θ)` silhouette to a dab texture — a white mask with
//! soft edges. The stamp node handles sizing, color, rotation, and
//! compositing. This separation means any procedural shape can be swapped in
//! without touching the stamping logic.
//!
//! Three shape algorithms are exposed via the `algorithm` enum param:
//!
//! - **Sine harmonic** — `r(θ) = 1 + A·sin(n·θ + φ)`. Symmetric bumps.
//! - **1D Perlin / value-noise fBm** — periodic value-noise summed over
//!   `octaves` with `persistence` falloff. Organic blobs.
//! - **Gielis Superformula** — single closed-form spanning circles, polygons,
//!   stars, flowers, and asteroids.
//!
//! Algorithms documented in `docs/brush/stamp-generation-algos.md`.
//!
//! ## Centroid alignment
//!
//! [`shaders/brush/stamp.wgsl`](../../../../shaders/brush/stamp.wgsl) maps the
//! dab texture's UV (0.5, 0.5) to the pen tip and pivots rotation around it.
//! Asymmetric `r(θ)` shapes (sine `n=1` kidney, low-`m` superformula, any
//! Perlin seed) put their geometric centroid *off* (0.5, 0.5), which would
//! make the brush drift away from the pen tip and rotate eccentrically. To
//! prevent this, this node numerically integrates the shape's centroid `(Cx,
//! Cy)` on the CPU per dab and passes it to the shader, which translates the
//! sample-space pole so the centroid lands at UV (0.5, 0.5).

use std::any::Any;

use crate::brush::dab_pool::DAB_REFERENCE_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::pipeline::{
    BrushPipelineEntry, BrushPipelineRegistration, BuildContext, DynamicUniformRing,
};
use crate::brush::wgsl_compile::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Pipeline ────────────────────────────────────────────────────────────

/// Uniform data for the circle mask generation shader.
///
/// Carries the algorithm choice (sine harmonic / 1D Perlin / Gielis
/// superformula), all per-algorithm shape parameters, and the CPU-computed
/// centroid offset that anchors the rendered shape's geometric centroid at
/// the texture centre.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CircleUniforms {
    pub softness: f32,    // 0-1 fraction of base radius
    pub algorithm: u32,   // 0 = sine harmonic, 1 = perlin/value-noise, 2 = superformula
    pub amplitude: f32,   // bump amplitude (sine, perlin) — fraction of base radius
    pub frequency: f32,   // bump count (sine.n, perlin.f, superformula.m)
    pub phase: f32,       // rotation in radians applied before r(θ) sample
    pub persistence: f32, // perlin: per-octave amplitude falloff
    pub seed: f32,        // perlin: rng seed
    pub octaves: u32,     // perlin: stacked frequency count
    pub n1: f32,          // superformula: overall sharpness
    pub n2: f32,          // superformula: bump rise
    pub n3: f32,          // superformula: bump fall
    pub base_radius: f32, // shrink factor so r_max stays inside the viewport
    pub centroid_x: f32,  // shape centroid in viewport-radius units
    pub centroid_y: f32,
    pub _pad: [f32; 2], // pad to 16-byte alignment
}

/// SDF circle mask pipeline.  Renders the circle mask to a dab texture
/// with REPLACE blend.  Also borrowed by the liquify node to synthesize
/// its preview ring.
pub struct CirclePipeline {
    pipeline: wgpu::RenderPipeline,
    ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
}

impl CirclePipeline {
    fn build(ctx: &BuildContext) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-circle"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../../../../shaders/brush/circle.wgsl").into(),
                ),
            });
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-circle-layout"),
                bind_group_layouts: &[ctx.uniform_bgl],
                immediate_size: 0,
            });
        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-circle"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            });
        let (ring, uniform_bind_group) = ctx.make_uniform_ring::<CircleUniforms>(
            "brush-circle-uniforms",
            "brush-circle-uniform-bg",
        );
        Self {
            pipeline,
            ring,
            uniform_bind_group,
        }
    }

    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub fn uniform_bind_group(&self) -> &wgpu::BindGroup {
        &self.uniform_bind_group
    }

    /// Write circle mask uniforms to the next ring slot.  Returns the
    /// dynamic byte offset for `set_bind_group`.
    pub fn write_uniforms(&self, queue: &wgpu::Queue, uniforms: &CircleUniforms) -> u32 {
        self.ring.write(queue, bytemuck::bytes_of(uniforms))
    }
}

impl BrushPipelineEntry for CirclePipeline {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ring(&self) -> Option<&DynamicUniformRing> {
        Some(&self.ring)
    }
}

fn circle_pipeline_reg() -> BrushPipelineRegistration {
    BrushPipelineRegistration {
        id: "circle",
        build: |ctx| Box::new(CirclePipeline::build(ctx)),
    }
}

// ── Node ────────────────────────────────────────────────────────────────

/// Algorithm-selector indices. Must match the `options` order in `register()`
/// and the branch order in `shaders/brush/circle.wgsl`.
const ALGO_SINE: u32 = 0;
const ALGO_PERLIN: u32 = 1;
const ALGO_SUPERFORMULA: u32 = 2;

/// Number of θ samples used for centroid integration. 256 keeps the centroid
/// accurate to sub-pixel even for high-frequency Perlin noise; the cost is a
/// few thousand flops per dab — negligible.
const CENTROID_SAMPLES: usize = 256;

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![circle_pipeline_reg()],
        node: NodeRegistration {
        type_id: "circle",
        category: "shape",
        display_name: "Circle",
        ports: vec![
            PortDef::input("softness", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Softness")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-feather")
                .with_description("Edge softness (0% = hard, 100% = feathered)"),
            // amplitude is meaningful for Sine and Perlin (modulates the
            // bumpy boundary); the Superformula's amplitude is implicit in
            // its n1/n2/n3 instead, so we hide this knob for it.
            PortDef::input("amplitude", BrushWireType::Scalar)
                .with_range(0.0, 0.5, 0.0)
                .with_natural_range(0.0, 0.5)
                .with_label("Amplitude")
                .with_unit(UnitType::Percent)
                .with_visible_when("algorithm", [ALGO_SINE as i32, ALGO_PERLIN as i32])
                .with_description("Bump amplitude as a fraction of the base radius."),
            // Frequency / phase are universal: the bump count, period, or
            // symmetry order — and the rotation around the shape's centre —
            // matter for every algorithm.
            PortDef::input("frequency", BrushWireType::Scalar)
                .with_range(1.0, 16.0, 6.0)
                .with_natural_range(1.0, 16.0)
                .with_step(1.0)
                .with_label("Frequency")
                .with_unit(UnitType::Raw)
                .with_description(
                    "Sine: number of bumps (n). Perlin: base period in cells per revolution. \
                     Superformula: symmetry order m. Must be an integer — \
                     non-integer values would create a seam at θ = ±π where the \
                     shape fails to close.",
                ),
            // No `natural_range`: radians are a unit, not a normalized
            // signal. `pen.tilt_direction → phase_input` is a unit-
            // preserving identity wire — values pass through raw and
            // sum with the user's `phase` offset. Users wanting
            // `random → phase_input` to span a full revolution must
            // pre-scale through `multiply`.
            PortDef::input("phase_input", BrushWireType::Scalar)
                .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                .with_label("Phase Input")
                .with_unit(UnitType::Degrees)
                .with_description(
                    "Per-dab phase, summed with `phase`. Wire `pen.tilt_direction` or `pen.drawing_angle` so the shape rotates with the pen.",
                ),
            PortDef::input("phase", BrushWireType::Scalar)
                .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                .with_label("Phase")
                .with_unit(UnitType::Degrees)
                // Orientation is part of shape identity (same rationale
                // as `stamp.rotation`); if the user exposes this knob,
                // the dab thumbnail should follow it.
                .persist_in_thumbnail()
                .with_description(
                    "Static rotation of the shape around its own centre, summed with `phase_input`. Route dynamic signals (tilt, drawing angle) into `phase_input` instead.",
                ),
            PortDef::input("persistence", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_label("Persistence")
                .with_unit(UnitType::Percent)
                .with_visible_when("algorithm", [ALGO_PERLIN as i32])
                .with_description("Per-octave amplitude falloff. Higher = rougher edge."),
            PortDef::input("seed", BrushWireType::Scalar)
                .with_range(0.0, 1024.0, 0.0)
                .with_natural_range(0.0, 1024.0)
                .with_label("Seed")
                .with_unit(UnitType::Raw)
                .with_visible_when("algorithm", [ALGO_PERLIN as i32])
                .with_description("RNG seed for the noise array."),
            PortDef::input("octaves", BrushWireType::Scalar)
                .with_range(1.0, 6.0, 3.0)
                .with_natural_range(1.0, 6.0)
                .with_label("Octaves")
                .with_unit(UnitType::Raw)
                .with_visible_when("algorithm", [ALGO_PERLIN as i32])
                .with_description("Number of stacked frequencies."),
            PortDef::input("n1", BrushWireType::Scalar)
                .with_range(0.1, 16.0, 1.0)
                .with_natural_range(0.1, 16.0)
                .with_label("n1")
                .with_unit(UnitType::Raw)
                .with_visible_when("algorithm", [ALGO_SUPERFORMULA as i32])
                .with_description("Overall fatness/sharpness."),
            PortDef::input("n2", BrushWireType::Scalar)
                .with_range(0.1, 16.0, 1.0)
                .with_natural_range(0.1, 16.0)
                .with_label("n2")
                .with_unit(UnitType::Raw)
                .with_visible_when("algorithm", [ALGO_SUPERFORMULA as i32])
                .with_description("Shape of bump rise."),
            PortDef::input("n3", BrushWireType::Scalar)
                .with_range(0.1, 16.0, 1.0)
                .with_natural_range(0.1, 16.0)
                .with_label("n3")
                .with_unit(UnitType::Raw)
                .with_visible_when("algorithm", [ALGO_SUPERFORMULA as i32])
                .with_description("Shape of bump fall."),
            PortDef::output("texture", BrushWireType::Texture)
                .with_description("Procedural mask texture"),
        ],
        params: &[ParamDef::Enum {
            name: "algorithm",
            options: &["Sine Harmonic", "Perlin Noise", "Superformula"],
            default: 0,
        }],
        is_gpu: true,
        },
    }
}

/// All shape parameters resolved from ports/params, in the units the shader
/// expects. Used by both centroid integration (CPU) and uniform packing.
#[derive(Copy, Clone)]
struct ShapeParams {
    algorithm: u32,
    amplitude: f32,
    frequency: f32,
    phase: f32,
    persistence: f32,
    seed: f32,
    octaves: u32,
    n1: f32,
    n2: f32,
    n3: f32,
}

impl ShapeParams {
    fn from_ctx(ctx: &EvalContext) -> Self {
        let algorithm = match ctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => (*v as u32).min(2),
            _ => 0,
        };
        ShapeParams {
            algorithm,
            amplitude: ctx.input_f32("amplitude").max(0.0),
            // Frequency must be an integer for r(θ) to close at θ = ±π.
            // Snap here too — the slider quantizes via PortDef::step, but a
            // wired-in modulator (curve, pen pressure) bypasses the slider
            // and would otherwise put a seam in the rendered shape.
            frequency: ctx.input_f32("frequency").round().max(1.0),
            phase: ctx.input_f32("phase") + ctx.input_f32("phase_input"),
            persistence: ctx.input_f32("persistence").clamp(0.0, 1.0),
            seed: ctx.input_f32("seed"),
            octaves: (ctx.input_f32("octaves").round() as u32).clamp(1, 6),
            n1: ctx.input_f32("n1").max(0.05),
            n2: ctx.input_f32("n2").max(0.05),
            n3: ctx.input_f32("n3").max(0.05),
        }
    }

    /// Conservative upper bound on `r(θ)` over the full revolution. Used to
    /// pick `base_radius = 1 / r_max_unit` so the rendered shape always fits
    /// inside the unit-radius viewport disc.
    ///
    /// `r_max_unit` is computed in the shape's natural units (where the
    /// unmodulated disc has `r = 1`). The shader then scales by `base_radius`
    /// so the final shape is bounded by the viewport.
    fn r_max_unit(&self) -> f32 {
        match self.algorithm {
            ALGO_SINE => 1.0 + self.amplitude,
            // Value-noise fBm in [0,1] remapped to [-1, 1] → ±1 swing,
            // scaled by amplitude. Worst case is r = 1 + amplitude.
            ALGO_PERLIN => 1.0 + self.amplitude,
            // Superformula radius is unbounded as n1 → 0; clamp to a
            // sane viewport-fitting upper bound. Sampling a few angles gives
            // a conservative estimate without doing a full optimization.
            // Sample over [-π, π) to match the shader's atan2 range — the
            // formula isn't 2π-periodic for n2 ≠ n3, so [0, 2π) and [-π, π)
            // produce different r values and we want the same range as the
            // rendered shape.
            ALGO_SUPERFORMULA => {
                let mut max_r: f32 = 0.0;
                for i in 0..32 {
                    let theta = -std::f32::consts::PI + (i as f32) * std::f32::consts::TAU / 32.0;
                    max_r = max_r.max(superformula_r(self, theta));
                }
                max_r.max(1.0)
            }
            _ => 1.0,
        }
    }
}

/// Polar radius `r(θ)` in the shape's natural units (unmodulated disc has
/// `r = 1`). Mirrors the same branching the WGSL shader does — see
/// `shaders/brush/circle.wgsl`. The duplication is intentional: it keeps the
/// CPU centroid integration consistent with the shader's rasterization, and
/// is small enough to verify by inspection (the centroid alignment test
/// catches drift if the formulas ever diverge).
fn r_theta(p: &ShapeParams, theta: f32) -> f32 {
    let theta = theta + p.phase;
    match p.algorithm {
        ALGO_SINE => 1.0 + p.amplitude * (p.frequency * theta).sin(),
        ALGO_PERLIN => {
            let t = theta / std::f32::consts::TAU;
            // Wrap to [0, 1) so we sample the periodic noise array at the
            // correct phase.
            let t = t - t.floor();
            // `fbm_1d` lives in [0, 1]; remap to [-1, 1] so amplitude scales
            // the same way it does for the sine path (sin lives in [-1, 1]
            // natively). Without the ×2 the slider's max felt half-strength
            // compared to sine.
            1.0 + p.amplitude * (2.0 * fbm_1d(t, p) - 1.0)
        }
        ALGO_SUPERFORMULA => superformula_r(p, theta),
        _ => 1.0,
    }
}

/// Gielis superformula with `a = b = 1`. `m` comes from `frequency`.
fn superformula_r(p: &ShapeParams, theta: f32) -> f32 {
    let m_quarter = p.frequency * theta * 0.25;
    let term_a = (m_quarter.cos().abs()).powf(p.n2);
    let term_b = (m_quarter.sin().abs()).powf(p.n3);
    let sum = term_a + term_b;
    if sum <= 0.0 {
        return 0.0;
    }
    sum.powf(-1.0 / p.n1)
}

/// Periodic 1D value-noise fBm sampled at `t ∈ [0, 1)`. `octaves` octaves
/// stacked with `persistence` amplitude falloff. Returns a value in `[0, 1]`.
///
/// Periodicity is preserved by wrapping each octave's cell index modulo the
/// integer period at that octave — so `fbm_1d(0, …) == fbm_1d(1, …)` exactly,
/// no seam where the polygon closes.
fn fbm_1d(t: f32, p: &ShapeParams) -> f32 {
    let mut sum = 0.0_f32;
    let mut norm = 0.0_f32;
    let mut amp = 1.0_f32;
    for o in 0..p.octaves {
        let freq = (p.frequency as i32).max(1) << o; // base * 2^o
        let x = t * freq as f32;
        let i = x.floor();
        let f = x - i;
        // Smoothstep interpolation between adjacent integer-cell hashes.
        let s = f * f * (3.0 - 2.0 * f);
        let a = hash1d(i.rem_euclid(freq as f32), p.seed);
        let b = hash1d((i + 1.0).rem_euclid(freq as f32), p.seed);
        sum += amp * (a * (1.0 - s) + b * s);
        norm += amp;
        amp *= p.persistence;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.5
    }
}

/// Deterministic integer bit-mix hash (Murmur3-style finalizer). Inputs are
/// the cell index `x` (always a small non-negative integer for our use) and
/// the user-facing `seed`. We use integer operations so the result is
/// bit-identical between Rust and the WGSL shader — the `fract(sin(x*K)*M)`
/// hash is tempting but `sin` precision differs between CPU's libm and the
/// GPU's transcendental approximation, and the `*43758` amplification turns
/// even sub-ULP sin drift into completely different noise arrays. The
/// centroid alignment test would catch that as a per-pixel mismatch.
fn hash1d(x: f32, seed: f32) -> f32 {
    let xi = x as u32;
    let si = seed as u32;
    let mut h = xi.wrapping_add(si.wrapping_mul(2654435761));
    h ^= h >> 16;
    h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    (h as f32) / (u32::MAX as f32)
}

/// Numerically integrate the shape's centroid in the algorithm's natural
/// units (where the unmodulated reference disc has `r = 1`). The shader adds
/// this directly to the pole-relative coordinate, which is also in natural
/// units — no `base_radius` scaling here, that conversion happens in the
/// shader's `(uv - 0.5) / base_radius` step.
///
/// For a region bounded by `r(θ)` in polar coordinates:
/// - area:     `A   = ½ ∫ r² dθ`
/// - centroid: `Cx  = (1/A)·(1/3) ∫ r³·cos(θ) dθ`
/// - centroid: `Cy  = (1/A)·(1/3) ∫ r³·sin(θ) dθ`
///
/// Integrated over θ ∈ [-π, π) to match the shader's `atan2` convention.
/// This matters for the superformula with `n2 ≠ n3` (and any other case
/// where `r(θ)` isn't strictly 2π-periodic): the shader's `atan2` returns
/// angles in (-π, π], so to compute the centroid of the *rendered* shape we
/// must sample r over the same range. Integrating over [0, 2π) instead would
/// silently evaluate a different shape and produce a centroid off by tens
/// of pixels.
fn integrate_centroid(p: &ShapeParams) -> (f32, f32) {
    let n = CENTROID_SAMPLES;
    let dtheta = std::f32::consts::TAU / n as f32;
    let mut area2 = 0.0_f32; // 2A
    let mut mx3 = 0.0_f32; // 3·∫ r³ cos
    let mut my3 = 0.0_f32; // 3·∫ r³ sin
    for i in 0..n {
        let theta = -std::f32::consts::PI + (i as f32 + 0.5) * dtheta; // mid-point on [-π, π)
        let r = r_theta(p, theta);
        let r2 = r * r;
        let r3 = r2 * r;
        area2 += r2 * dtheta;
        mx3 += r3 * theta.cos() * dtheta;
        my3 += r3 * theta.sin() * dtheta;
    }
    let area = 0.5 * area2;
    if area.abs() < 1e-6 {
        return (0.0, 0.0);
    }
    let cx = (mx3 / 3.0) / area;
    let cy = (my3 / 3.0) / area;
    (cx, cy)
}

pub struct CircleEvaluator;

impl BrushNodeEvaluator for CircleEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let softness = ctx.input_f32("softness");
        let shape = ShapeParams::from_ctx(ctx);

        // Pick `base_radius` so the worst-case `r(θ)` lands inside the
        // viewport disc with a small AA margin. UV space goes 0..1 with the
        // centre at 0.5, so the maximum UV-distance to a viewport edge along
        // the axes is 0.5; the 0.498 figure preserves the same 2-pixel-ish
        // margin the original hard-circle shader used.
        let r_max = shape.r_max_unit().max(1e-3);
        let base_radius = 0.498 / r_max;

        let (cx, cy) = integrate_centroid(&shape);

        let handle = gpu.dab_pool.acquire(gpu.device);
        let dab_view = gpu.dab_pool.view(handle);

        let uniforms = CircleUniforms {
            softness,
            algorithm: shape.algorithm,
            amplitude: shape.amplitude,
            frequency: shape.frequency,
            phase: shape.phase,
            persistence: shape.persistence,
            seed: shape.seed,
            octaves: shape.octaves,
            n1: shape.n1,
            n2: shape.n2,
            n3: shape.n3,
            base_radius,
            centroid_x: cx,
            centroid_y: cy,
            _pad: [0.0; 2],
        };
        let circle = gpu.pipelines.get::<CirclePipeline>("circle");
        let offset = circle.write_uniforms(gpu.queue, &uniforms);

        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-circle"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dab_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            let size = DAB_REFERENCE_SIZE as f32;
            pass.set_viewport(0.0, 0.0, size, size, 0.0, 1.0);
            pass.set_pipeline(circle.pipeline());
            pass.set_bind_group(0, circle.uniform_bind_group(), &[offset]);
            pass.draw(0..3, 0..1);
        }

        vec![("texture".into(), ScalarValue::Texture(handle))]
    }

    /// Emit a per-fragment coverage function for the compiled brush
    /// path. The `texture` output is bridged to a scalar coverage
    /// value: downstream nodes (notably `stamp`) substitute the
    /// function call into their `tip` input as if it were sampling a
    /// procedurally-generated mask. No actual texture is allocated.
    ///
    /// `params.algorithm` is read from the node param at compile time
    /// (constant per brush). Per-port shape inputs (`amplitude`,
    /// `phase`, `seed`, etc.) become input expressions — wired to
    /// dab-record fields when modulated, literals when not.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("texture") {
            return Ok(wgsl);
        }

        let algorithm = match cctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => (*v as u32).min(2),
            _ => 0,
        };
        let amplitude = cctx.input("amplitude").as_f32();
        let frequency = cctx.input("frequency").as_f32();
        let phase = cctx.input("phase").as_f32();
        let phase_input = cctx.input("phase_input").as_f32();
        let persistence = cctx.input("persistence").as_f32();
        let seed = cctx.input("seed").as_f32();
        let octaves = cctx.input("octaves").as_f32();
        let n1 = cctx.input("n1").as_f32();
        let n2 = cctx.input("n2").as_f32();
        let n3 = cctx.input("n3").as_f32();
        let softness = cctx.input("softness").as_f32();

        // Emit the shape evaluation as an inline block inside
        // `fs_main` rather than a top-level function — the input
        // expressions reference `d.<field>` and `u.<field>` which are
        // only in scope inside the fragment shader body. Using a
        // block-let preserves a single `let` binding name downstream
        // nodes can substitute.
        let params_ident = cctx.ident("circle_params");
        let shape_ident = cctx.ident("circle_shape");
        let body = format!(
            "    let {params_ident}: ShapeParams = ShapeParams(\n\
             \x20       {algorithm}u,\n\
             \x20       max(({amplitude}), 0.0),\n\
             \x20       max(round(({frequency})), 1.0),\n\
             \x20       ({phase}) + ({phase_input}),\n\
             \x20       clamp(({persistence}), 0.0, 1.0),\n\
             \x20       ({seed}),\n\
             \x20       clamp(u32(round(({octaves}))), 1u, 6u),\n\
             \x20       max(({n1}), 0.05),\n\
             \x20       max(({n2}), 0.05),\n\
             \x20       max(({n3}), 0.05),\n\
             \x20   );\n\
             \x20   let {shape_ident}_r_at: f32 = shape_r_theta({params_ident}, theta);\n\
             \x20   let {shape_ident}_soft: f32 = clamp(({softness}), 0.0, 1.0);\n\
             \x20   let {shape_ident}_r_solid: f32 = {shape_ident}_r_at * (1.0 - {shape_ident}_soft);\n\
             \x20   var {shape_ident}: f32 = 0.0;\n\
             \x20   if (local_dist >= {shape_ident}_r_at) {{ {shape_ident} = 0.0; }}\n\
             \x20   else if (local_dist <= {shape_ident}_r_solid) {{ {shape_ident} = 1.0; }}\n\
             \x20   else {{ {shape_ident} = clamp(({shape_ident}_r_at - local_dist) / max({shape_ident}_r_at - {shape_ident}_r_solid, 1e-5), 0.0, 1.0); }}\n",
        );
        wgsl.body = body;
        wgsl.outputs.insert("texture".into(), shape_ident);
        Ok(wgsl)
    }
}
