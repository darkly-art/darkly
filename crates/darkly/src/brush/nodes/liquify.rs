//! Liquify terminal: per-dab displacement-field warp with a per-brush
//! compiled WGSL shader.
//!
//! Rides the shared [read-mirror terminal](crate::brush::read_mirror_terminal)
//! infrastructure (the per-brush pipeline, dab-meta queue, flush loop,
//! and `copy_origin` plumbing it shares with `smudge` and `blur`). This
//! file owns only what's liquify-specific: the read half-extent and the
//! variant WGSL (including the softness falloff helper).
//!
//! Unlike its read-mirror siblings, liquify's scratch holds a
//! [warp field](crate::brush::warp_field) rather than colour. Per dab the
//! fragment shader advects the accumulated displacement and adds this
//! dab's own; it never touches a pixel. The picture is produced once, at
//! commit, by sampling the pre-stroke snapshot through the finished
//! field.
//!
//! That is not a shortcut around per-dab compounding, it is how the
//! compounding is made lossless. Later dabs still displace content
//! earlier dabs displaced: the `field(p + nv)` term reads the previous
//! field at the displaced location and carries it along, which composes
//! the two maps exactly. What it does *not* do is resample the picture
//! each time: at 4 px spacing under a 77 px brush that was ~38 chained
//! bilinear filters per swipe, and a chain of bilinear filters is a
//! low-pass cascade. Detail is now independent of dab count.
//!
//! The per-dab serialization is therefore still semantically required,
//! not a perf bug: dab *n+1* must read dab *n*'s field.
//!
//! Inherited caveat, unchanged by the field model: the read-mirror fetch
//! addresses the mirror by `textureDimensions`, while only the copied
//! `copy_w × copy_h` sub-rect of that lazily-grown texture is valid
//! (`scratch.rs`), so a dab clipped at the layer edge can address stale
//! texels. The helper clamps to the texture, not to the valid rect.
//!
//! Displacement magnitude is `strength × |pen.motion|`: the cursor's
//! per-dab travel scaled by strength. So:
//!   * `strength = 1` locks pixels to the cursor (per-dab push =
//!     per-dab cursor motion);
//!   * `strength < 1` produces a strength-fraction drag.
//!
//! Pen speed enters only via dab density along the path; the per-dab
//! push is identical for slow and fast drags.
//!
//! ## Why spacing is proportional, not pinned
//!
//! Dab spacing uses the ordinary proportional rule
//! ([`SpacingConfig`](crate::brush::spacing::SpacingConfig)) at
//! [`LIQUIFY_SPACING_RATIO`], rather than the flat pixel floor it once
//! carried. Total displacement over a drag does not depend on spacing:
//! per dab it is `strength × |motion| = strength × spacing`, and a drag
//! of length `L` places `L / spacing` dabs, so the total is
//! `strength × L`: spacing cancels. It only sets how finely the warp is
//! discretised.
//!
//! That is a property of accumulating a *field*. Under the per-dab image
//! warp this replaced, spacing also cancelled geometrically, but each dab
//! cost a resample, so the dab count could not be traded for performance
//! without trading away detail, and the spacing was pinned flat at 4 px.
//! Pinned spacing makes cost `O(radius²)` per unit of travel: dab count
//! stays constant while each dab's mirror copy and fragment pass grow
//! with the disc. Proportional spacing makes it `O(radius)`.
//!
//! The ratio is bounded by banding, not by intensity. Measured on a
//! straight drag at radius 76.8: peak displacement moves 45.01 → 45.11 px
//! (+0.2 %) from 4 px to 8 px spacing, then 45.52 at 16 px and 48.71
//! (+8 %, visibly stepped) at 32 px. Spacing up to ~0.1 × radius is
//! faithful; beyond that the discretisation starts showing.
//!
//! GIMP's warp tool reaches the same place: `step = effect_size ×
//! stroke_spacing / 100` (`app/tools/gimpwarptool.c:432`), spacing
//! proportional to brush size, at a comparable default density.
//!
//! ## Softness waveshape
//!
//! User-facing slider: `0 = hard` (uniform displacement across the
//! disc, square edge) ↔ `1 = soft` (sharp peak at the brush centre,
//! near-zero past the half-radius; only the cursor itself drags
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
use crate::gpu::preview::{PreviewBackdrop, PreviewStaging};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

// ── Constants ───────────────────────────────────────────────────────────

/// Dab spacing for the Liquify brush, as a fraction of dab **diameter**:
/// the value `brushes/liquify.yaml` sets on `brush_settings.spacing`.
///
/// 0.05 of diameter is 0.1 of radius, the density the module doc's
/// measurements put at the edge of faithful: at that spacing the warp is
/// within 0.2 % of a 4 px reference, and it holds the per-unit-travel
/// cost at `O(radius)` instead of the `O(radius²)` a pinned pixel
/// spacing forced.
///
/// Declared here rather than only in the YAML because it is a property
/// of how this terminal behaves, and because the module doc's reasoning
/// is what justifies the number. Keep the two in step.
pub const LIQUIFY_SPACING_RATIO: f32 = 0.05;

/// Per-dab strength below which the dab is dropped, since the dab's
/// displacement collapses to zero, so advecting the field by it and
/// adding it back is an identity write.
const STRENGTH_EPSILON: f32 = 1.0e-4;

/// Brush radius below which the dab is dropped, since sub-pixel discs warp
/// nothing visible.
const MIN_RADIUS_PX: f32 = 1.0;

/// Cumulative stroke distance below which liquify silently skips the dab.
/// The stroke's opening dabs have zero or sub-pixel per-dab motion, so their
/// displacement is nil and rendering them is wasted work.
const MIN_DISTANCE_PX: f32 = 0.5;

pub const TYPE_ID: &str = "liquify";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![read_mirror_pipeline_reg("liquify")],
        evaluator: || Box::new(LiquifyEvaluator),
        // A transparent clear *is* a zero field: no displacement
        // anywhere, so the first resolve reproduces the pre-stroke image
        // exactly. Seeding from pre-stroke would be meaningless: the
        // scratch holds offsets, not colour.
        lifecycle: crate::brush::node::Lifecycle::ClearScratchToTransparent,
        scratch_format: crate::brush::warp_field::FIELD_FORMAT,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Liquify",
            description: "Output that pushes existing canvas pixels around in the direction of the stroke, like a warp brush.",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Where to apply the warp"),
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
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_description(
                        "Per-touch size multiplier (wire pressure here for pressure-sensitive size). Multiplies onto the brush's base size, owned by pen_input.",
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
                    "Per-fragment shape mask (typically wired from circle.mask); \
                     defaults to 1.0 (uniform inside the disc) when unwired.",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Size of the affected area"),
            ],
            is_gpu: true,
            is_terminal: true,
            supports_erase: false,
            preview_staging: Some(PreviewStaging {
                icon: "tabler:ripple",
                backdrop: PreviewBackdrop::Stripes,
            }),
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

        // Three early-outs: skip stationary or sub-pixel dabs whose warp
        // would be a no-op.
        if radius < MIN_RADIUS_PX || strength < STRENGTH_EPSILON || distance < MIN_DISTANCE_PX {
            return None;
        }

        // Symmetric read region: disc inflated by `displacement` per axis
        // so the warped sample at
        // `target_pos - motion × strength × falloff(d)` always lies
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

        // `mask` defaults to 1.0 when unwired: uniform warp inside the
        // disc.
        let mask_expr = if cctx.input_is_wired("mask") {
            cctx.input("mask").as_f32()
        } else {
            "1.0".to_string()
        };
        let strength_expr = cctx.input("strength").as_f32();
        let softness_expr = cctx.input("softness").as_f32();
        let motion_expr = cctx.input("motion").as_vec2();

        // Per-node falloff fn: suffixed by node id so two liquify
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
             }}\n\
             {}\n",
            crate::brush::warp_field::FIELD_HELPERS_WGSL,
        );

        // Fragment body: `local_dist` and `target_pos` come from the
        // framework wrapper; the framework already discards past
        // `d.bbox_target_px`. We additionally discard past
        // `local_dist >= 1.0` so the warp stays outside the disc alone.
        // The falloff helper takes `0 = spike` / `1 = square`. The
        // user-facing slider is labelled "Softness" with the opposite
        // intuition: `1 = soft / feathery`, `0 = hard / sharp`. Invert
        // before passing to the helper so the slider matches the label.
        //
        // `sel` and `warp_mask` scale the *displacement*, never the
        // result. A geometric operation that cross-faded warped against
        // unwarped colour would be a literal double exposure inside a
        // soft selection edge; here every output pixel remains exactly
        // one sample of the source, and a masked-out fragment simply
        // contributes a zero offset (leaving the accumulated field
        // untouched).
        // Pixels are pushed straight along the per-dab motion vector: the
        // signed direction *and* magnitude of where the cursor actually went.
        let offset_expr = "-motion_vec * strength * f * sel * warp_mask";
        wgsl.body = format!(
            "    if (local_dist >= 1.0) {{ discard; }}\n\
             \x20   let warp_mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   let strength = clamp({strength_expr}, 0.0, 1.0);\n\
             \x20   let softness = clamp({softness_expr}, 0.0, 1.0);\n\
             \x20   let falloff_param = 1.0 - softness;\n\
             \x20   let motion_vec = {motion_expr};\n\
             \x20   let f = {falloff_fn}(local_dist, falloff_param);\n\
             {}",
            crate::brush::warp_field::advect_wgsl(offset_expr, copy_origin_field),
        );

        Ok(wgsl)
    }

    /// Preview body emits the falloff disc so scrubbing the softness
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
