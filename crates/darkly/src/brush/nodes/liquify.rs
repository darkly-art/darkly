//! Liquify terminal — per-dab fragment-pass warp with a per-brush
//! compiled WGSL shader.
//!
//! Rides the shared [read-mirror terminal](crate::brush::read_mirror_terminal)
//! infrastructure (the per-brush pipeline, dab-meta queue, flush loop,
//! and `copy_origin` plumbing it shares with `smudge` and `blur`). This
//! file owns only what's liquify-specific: the read half-extent and the
//! variant WGSL (including the softness falloff helper).
//!
//! Per dab the fragment shader samples the scratch read mirror at a
//! *displaced* UV inside a circular brush disc and writes the warped
//! sample back into the scratch. Successive dabs compound because each
//! reads the cumulatively-warped scratch — the per-dab serialization is
//! semantically required, not a perf bug.
//!
//! Displacement magnitude is `strength × |pen.motion|` — the cursor's
//! per-dab travel scaled by strength. With the Liquify brush's fixed
//! `pen_input.spacing_min_px = LIQUIFY_SPACING_PX`, `|motion|` is the
//! same constant at any brush size, so:
//!   * `strength = 1` locks pixels to the cursor (per-dab push =
//!     per-dab cursor motion);
//!   * `strength < 1` produces a strength-fraction drag;
//!   * brush size controls only the warped *extent* (the disc), never
//!     the *intensity*.
//!
//! Pen speed enters only via dab density along the path; the per-dab
//! push is identical for slow and fast drags. **Liquify is deliberately
//! size-invariant** — the size slider scales the warped extent, not the
//! push strength.
//!
//! ## Softness waveshape
//!
//! User-facing slider: `0 = hard` (uniform displacement across the
//! disc, square edge) ↔ `1 = soft` (sharp peak at the brush centre,
//! near-zero past the half-radius — only the cursor itself drags
//! pixels). Internally the falloff helper takes the *opposite*
//! convention (`0 = spike → 1 = square`), and the WGSL body inverts
//! the slider value before passing it in. The mapping the user sees:
//!   0    → uniform / square     (helper input `1.0`)
//!   0.5  → sine                  (helper input `0.5`)
//!   0.6  → linear saw            (helper input `0.4`)
//!   1    → spike (`pow(1-d, 8)`) (helper input `0.0`)

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::read_mirror_terminal::{
    self as rmt, read_mirror_pipeline_reg, ReadMirrorTerminal,
};
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Dab spacing for the Liquify brush, in canvas pixels. The brush
/// pins `pen_input.spacing_min_px` to this value (and sets ratio to
/// zero) so spacing stays fixed at any brush size. Per-dab
/// displacement is then `strength × |pen.motion| ≈ strength ×
/// LIQUIFY_SPACING_PX`, which makes:
///   * `strength = 1` lock pixels to the cursor (per-dab push equals
///     per-dab cursor motion);
///   * `strength = 0.5` lag the cursor by 50% (the "drag" feel);
///   * the absolute pixel push size-invariant — the size slider
///     controls the warped *extent* (the disc), not the *intensity*.
///
/// Tuned to 4 px: tight enough for smooth-looking warps without dab
/// banding, large enough not to blow up the dab count at huge
/// brushes (perf scales with `diameter / spacing`).
pub const LIQUIFY_SPACING_PX: f32 = 4.0;

/// Per-dab strength below which the dab is dropped — `mix(orig, warped, _·sel)`
/// collapses to identity and the per-dab pass would be a no-op.
const STRENGTH_EPSILON: f32 = 1.0e-4;

/// Brush radius below which the dab is dropped — sub-pixel discs warp
/// nothing visible.
const MIN_RADIUS_PX: f32 = 1.0;

/// Cumulative stroke distance below which liquify silently skips the
/// first dab. Without this, a stationary click would warp rightward
/// (default `drawing_angle = 0`).
const MIN_DISTANCE_PX: f32 = 0.5;

pub const TYPE_ID: &str = "liquify";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![read_mirror_pipeline_reg("liquify")],
        evaluator: || Box::new(LiquifyEvaluator),
        lifecycle: crate::brush::node::Lifecycle::SeedScratchFromPreStroke,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Liquify",
            description: "Output that pushes existing canvas pixels around in the direction of the stroke, like a warp brush.",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Where to apply the warp"),
                // No `natural_range`: radians are a unit, not a normalized
                // signal. `pen.drawing_angle → direction` (canonical wire)
                // is a unit-preserving identity.
                PortDef::input("direction", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_description("Direction to push pixels"),
                PortDef::input("distance", BrushWireType::Scalar)
                    .with_description("How far the pen has traveled along the stroke"),
                // Per-dab cursor motion in canvas pixels. Wire from
                // `pen.motion`; the magnitude becomes the per-dab
                // displacement scale (`strength × |motion|`) so 100%
                // strength locks pixels to the cursor and 50% lets
                // them drag half-step behind, regardless of brush
                // size. When unwired, defaults to (0, 0) → no warp.
                PortDef::input("motion", BrushWireType::Vec2)
                    .with_description(
                        "Per-dab cursor motion vector. Magnitude sets \
                         the per-dab displacement scale.",
                    ),
                PortDef::input("size_input", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size Input")
                    .with_unit(UnitType::Percent)
                    .with_description(
                        "Per-touch size multiplier (wire pressure here for pressure-sensitive size).",
                    ),
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 4.0, 0.3)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:up-right-and-down-left-from-center")
                    .exposed()
                    .with_preview_value(0.1)
                    .with_description(
                        "Brush size. Can go above 100% for large-area warps (capped at 400%).",
                    ),
                PortDef::input("strength", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Strength")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:gauge-high")
                    .exposed()
                    .with_description("How far pixels are pushed by each brush touch"),
                PortDef::input("softness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Softness")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:wave-square")
                    .exposed()
                    .with_description(
                        "Edge shape. Low values concentrate the warp at the brush center; \
                         high values spread it evenly across the brush.",
                    ),
                // Optional brush-shape modulation. If wired, the warp
                // strength multiplies by the upstream coverage. If
                // unwired, defaults to 1.0 (uniform inside the disc).
                PortDef::input("mask", BrushWireType::Scalar).with_description(
                    "Per-fragment shape mask (typically wired from shape.mask); \
                     defaults to 1.0 (uniform inside the disc) when unwired.",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Size of the affected area"),
            ],
            params: &[],
            is_gpu: true,
            is_terminal: true,
            supports_erase: false,
            preview_fallback_icon: Some("tabler:ripple"),
        },
    }
}

pub struct LiquifyEvaluator;

impl ReadMirrorTerminal for LiquifyEvaluator {
    const PIPELINE_ID: &'static str = "liquify";
    const LABEL: &'static str = "liquify";

    fn read_half(&self, ctx: &EvalContext, radius: f32, _bbox_radius: f32) -> Option<[f32; 2]> {
        let strength = ctx.input_f32("strength").clamp(0.0, 1.0);
        let distance = ctx.input_f32("distance");
        let motion = ctx.input("motion").as_vec2();
        let motion_mag = (motion[0] * motion[0] + motion[1] * motion[1]).sqrt();

        // Three early-outs — skip stationary or sub-pixel dabs whose warp
        // would be a no-op.
        if radius < MIN_RADIUS_PX || strength < STRENGTH_EPSILON || distance < MIN_DISTANCE_PX {
            return None;
        }

        // Symmetric read region — disc inflated by `displacement` per axis
        // so the warped sample at
        // `target_pos - direction × displacement × falloff(d)` always lies
        // inside the mirror snapshot (the bilinear sampler reaches into
        // the inflation margin too). `displacement = strength × |motion|`,
        // recomputed identically by the shader from motion + strength.
        let displacement = motion_mag * strength;
        let read_half = radius + displacement;
        Some([read_half, read_half])
    }

    fn compile_body(
        &self,
        cctx: &CompileWgslCtx,
        copy_origin_field: &str,
    ) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();

        // `mask` defaults to 1.0 when unwired — uniform warp inside the
        // disc.
        let mask_expr = if cctx.input_is_wired("mask") {
            cctx.input("mask").as_f32()
        } else {
            "1.0".to_string()
        };
        let strength_expr = cctx.input("strength").as_f32();
        let softness_expr = cctx.input("softness").as_f32();
        let direction_expr = cctx.input("direction").as_f32();
        let motion_expr = cctx.input("motion").as_vec2();

        // Per-node falloff fn — suffixed by node id so two liquify
        // terminals (hypothetical) in the same brush don't collide.
        let falloff_fn = cctx.ident("liquify_falloff");
        wgsl.decls = format!(
            "fn {falloff_fn}(d_norm: f32, softness: f32) -> f32 {{\n\
             \x20   let saw  = 1.0 - d_norm;\n\
             \x20   let sine = 0.5 + 0.5 * cos(3.14159265 * d_norm);\n\
             \x20   let saw_break  = 0.4;\n\
             \x20   let sine_break = 0.5;\n\
             \x20   let k_max      = 8.0;\n\
             \x20   if softness <= saw_break {{\n\
             \x20       let t = softness / saw_break;\n\
             \x20       let k = mix(k_max, 1.0, t);\n\
             \x20       return pow(max(saw, 0.0), k);\n\
             \x20   }} else if softness <= sine_break {{\n\
             \x20       let t = (softness - saw_break) / (sine_break - saw_break);\n\
             \x20       return mix(saw, sine, t);\n\
             \x20   }} else {{\n\
             \x20       let t = (softness - sine_break) / (1.0 - sine_break);\n\
             \x20       return mix(sine, 1.0, t);\n\
             \x20   }}\n\
             }}\n"
        );

        // Fragment body: `local_dist` and `target_pos` come from the
        // framework wrapper; the framework already discards past
        // `d.bbox_target_px`. We additionally discard past
        // `local_dist >= 1.0` so the warp stays inside the disc.
        // The falloff helper takes `0 = spike` / `1 = square`. The
        // user-facing slider is labelled "Softness" with the opposite
        // intuition — `1 = soft / feathery`, `0 = hard / sharp`. Invert
        // before passing to the helper so the slider matches the label.
        wgsl.body = format!(
            "    if (local_dist >= 1.0) {{ discard; }}\n\
             \x20   let warp_mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   let strength = clamp({strength_expr}, 0.0, 1.0);\n\
             \x20   let softness = clamp({softness_expr}, 0.0, 1.0);\n\
             \x20   let falloff_param = 1.0 - softness;\n\
             \x20   let direction_angle = {direction_expr};\n\
             \x20   let motion_vec = {motion_expr};\n\
             \x20   let f = {falloff_fn}(local_dist, falloff_param);\n\
             \x20   let dir = vec2<f32>(cos(direction_angle), sin(direction_angle));\n\
             \x20   let displacement = length(motion_vec) * strength;\n\
             \x20   let source_pos = target_pos - dir * displacement * f;\n\
             \x20   let mirror_dims = vec2<f32>(textureDimensions(scratch_mirror_tex));\n\
             \x20   let copy_uv    = (source_pos - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let warped     = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, copy_uv,    0.0);\n\
             \x20   let original_uv = (target_pos  - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let original   = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, original_uv, 0.0);\n\
             \x20   return mix(original, warped, sel * warp_mask);\n",
        );

        Ok(wgsl)
    }

    /// Preview body — emit the falloff disc so scrubbing the softness
    /// slider visibly reshapes the cursor (a side-effect of reusing the
    /// same `falloff_fn` the stroke decls emit). The stroke body's
    /// `scratch_mirror` bindings are omitted in preview mode.
    ///
    /// The overlay's `KIND_MASKED_STAMP` reads only the `.r` channel of
    /// this mask as coverage; the displayed colour comes from
    /// `fs_snapshot`'s background-shift math, not anything written here.
    /// So `.r = f` puts liquify's peak coverage on par with paint's at
    /// the centre.
    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let softness_expr = cctx.input("softness").as_f32();
        let falloff_fn = cctx.ident("liquify_falloff");
        wgsl.body = format!(
            "    if (local_dist >= 1.0) {{ discard; }}\n\
             \x20   let softness = clamp({softness_expr}, 0.0, 1.0);\n\
             \x20   let f = {falloff_fn}(local_dist, 1.0 - softness);\n\
             \x20   return vec4<f32>(f, f, f, f);\n"
        );
        Ok(wgsl)
    }
}

impl BrushNodeEvaluator for LiquifyEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        rmt::evaluate_gpu(self, ctx, gpu)
    }

    fn flush_dabs(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        rmt::flush_dabs::<Self>(gpu)
    }

    fn commit(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        rmt::commit(gpu)
    }

    fn render_cursor_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        rmt::render_cursor_preview(ctx, gpu)
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        rmt::compile_wgsl(self, cctx)
    }

    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        ReadMirrorTerminal::compile_cursor_preview_body(self, cctx)
    }
}
