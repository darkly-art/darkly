//! Procedural shape coverage GPU node.
//!
//! Compile-only: contributes a per-fragment scalar coverage expression
//! (`f32` in `[0, 1]`) to the brush's compiled WGSL via [`compile_wgsl`].
//! Downstream consumers (stamp's AlphaMask mode, watercolor's
//! `mask`, smudge's `mask`, liquify's `mask`) inline
//! the expression directly into their fragment body — no dab texture,
//! no separate render pass.
//!
//! Three shape algorithms are exposed via the `algorithm` enum param:
//!
//! - **Sine harmonic** — `r(θ) = 1 + A·sin(n·θ + φ)`. Symmetric bumps.
//! - **1D Perlin / value-noise fBm** — periodic value-noise summed over
//!   `octaves` with `persistence` falloff. Organic blobs.
//! - **Gielis Superformula** — single closed-form spanning circles, polygons,
//!   stars, flowers, and asteroids.
//!
//! Algorithms documented in `docs/brush/stamp-generation-algos.md`. The
//! shared `r(θ)` math lives in `shaders/brush/_shape.wgsl` and is
//! spliced into every compiled brush that consumes a shape.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl_compile::{CompileWgslCtx, ExtentContribution, ExtentCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Node ────────────────────────────────────────────────────────────────

/// Algorithm-selector indices. Must match the `options` order in `register()`
/// and the branch order in `shaders/brush/_shape.wgsl`.
const ALGO_SINE: u32 = 0;
const ALGO_PERLIN: u32 = 1;
const ALGO_SUPERFORMULA: u32 = 2;

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![],
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

/// Gielis superformula with `a = b = 1`. Used only by
/// [`CircleEvaluator::extent`] to bound the dab footprint when the
/// shape is set to Superformula; the per-fragment math lives in
/// `shaders/brush/_shape.wgsl` and the compiled brush splices it in
/// via [`CircleEvaluator::compile_wgsl`].
fn superformula_r(theta: f32, frequency: f32, n1: f32, n2: f32, n3: f32) -> f32 {
    let m_quarter = frequency * theta * 0.25;
    let term_a = (m_quarter.cos().abs()).powf(n2);
    let term_b = (m_quarter.sin().abs()).powf(n3);
    let sum = term_a + term_b;
    if sum <= 0.0 {
        return 0.0;
    }
    sum.powf(-1.0 / n1)
}

pub struct CircleEvaluator;

impl BrushNodeEvaluator for CircleEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
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
        //
        // Edge coverage mirrors `shaders/brush/circle.wgsl`:
        // smoothstep over a constant softness band (in unit-disc /
        // natural units, independent of `r_at`), with a 0.004 AA
        // floor so softness == 0 still produces a one-pixel-ish
        // anti-aliased boundary instead of jagged stair-steps.
        //
        // A linear ramp scaled by `r_at` (what the original draft did)
        // makes the falloff width vary along the perlin edge —
        // outward bumps get softer than inward dips — which reads as
        // "wonky" and exaggerates the noise band when the dab is
        // large.
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
             \x20   let {shape_ident}_band: f32 = max(clamp(({softness}), 0.0, 1.0), 0.004);\n\
             \x20   let {shape_ident}: f32 = 1.0 - smoothstep({shape_ident}_r_at - {shape_ident}_band, {shape_ident}_r_at, local_dist);\n",
        );
        wgsl.body = body;
        wgsl.outputs.insert("texture".into(), shape_ident);
        Ok(wgsl)
    }

    /// Worst-case multiplier on the upstream dab extent. The shape's
    /// `r(θ)` is in units of the upstream radius (compiled-path
    /// `local_dist` is `length(local_uv)` where
    /// `local_uv = local * d.inv_radius_target_px`), so the dab
    /// footprint stretches by the algorithm's
    /// `r_max`. Mirrors [`ShapeParams::r_max_unit`] but uses
    /// [`ExtentCtx::port_max_value`] so the bound covers every value
    /// any wire can deliver, not just the per-dab realisation.
    fn extent(&self, ctx: &ExtentCtx) -> ExtentContribution {
        let algorithm = match ctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => (*v as u32).min(2),
            _ => 0,
        };
        let factor = match algorithm {
            // r(θ) = 1 + A·sin(...) for sine, and 1 + A·(2·fbm - 1)
            // for perlin (fbm ∈ [0, 1] → swing in [-1, 1]) — both
            // peak at 1 + amplitude_max.
            ALGO_SINE | ALGO_PERLIN => 1.0 + ctx.port_max_value("amplitude").max(0.0),
            // Superformula's r is unbounded as n1 → 0; the best we
            // can do without per-dab knowledge is a numerical scan
            // using port_max for the three n-knobs and the port's
            // frequency_max. Matches the per-dab `r_max_unit` shape
            // exactly, only with worst-case wire values plugged in.
            ALGO_SUPERFORMULA => {
                let frequency = ctx.port_max_value("frequency").max(1.0).round();
                let n1 = ctx.port_max_value("n1").max(0.05);
                let n2 = ctx.port_max_value("n2").max(0.05);
                let n3 = ctx.port_max_value("n3").max(0.05);
                let mut max_r: f32 = 0.0;
                for i in 0..32 {
                    let theta = -std::f32::consts::PI + (i as f32) * std::f32::consts::TAU / 32.0;
                    max_r = max_r.max(superformula_r(theta, frequency, n1, n2, n3));
                }
                max_r.max(1.0)
            }
            _ => 1.0,
        };
        ExtentContribution::Multiply(factor)
    }
}
