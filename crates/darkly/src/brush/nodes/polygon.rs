//! Procedural regular-polygon coverage GPU node — a straight-edged brush tip.
//!
//! Compile-only: contributes a per-fragment scalar coverage expression
//! (`f32` in `[0, 1]`) to the brush's compiled WGSL via [`compile_wgsl`],
//! inlined by downstream consumers (`stamp.tip`, …) exactly like
//! [`super::circle`]'s `mask` — no dab texture, no separate pass.
//!
//! Unlike the [`super::circle`] family, which measures coverage from a
//! polar radius `r(θ)` corrected by a per-angle gradient, polygon coverage
//! is a **true signed-distance field**. A polar approximation folds its
//! level sets into a caustic at a rounded corner (the iso-distance contours
//! there are arcs about the fillet centre, not radial from the dab centre);
//! an exact SDF has parallel iso-contours everywhere, so rounding and
//! softness compose cleanly at any values.
//!
//! Credit: the regular-polygon SDF is Inigo Quilez's `sdRegularPolygon`,
//! <https://iquilezles.org/articles/distfunctions2d/>. The corner radius
//! is applied with the standard SDF rounding operator (`sd - ρ`).

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, ExtentContribution, ExtentCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub const TYPE_ID: &str = "polygon";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![],
        evaluator: || Box::new(PolygonEvaluator),
        lifecycle: crate::brush::node::Lifecycle::None,
        node: NodeRegistration {
            type_id: TYPE_ID,
            // Shared UI grouping with `circle` and `stamp` — the tip
            // generators — not a type id (see the Modularity Principle).
            category: "shape",
            display_name: "Polygon",
            description: "Procedural brush-tip silhouette — a rounded regular polygon (SDF-based).",
            ports: vec![
                // Side count. An integer knob (a fractional polygon can't
                // close); wirable like every other input, clamped to a convex
                // minimum of 3 in the emitted WGSL.
                PortDef::input("points", BrushWireType::Int)
                    .with_range(3.0, 16.0, 5.0)
                    .with_natural_range(3.0, 16.0)
                    .with_value(InputValue::Int(5))
                    .with_step(1.0)
                    .with_label("Points")
                    .with_unit(UnitType::Raw)
                    .with_description("Number of sides (min 3)."),
                // Corner radius, applied as the SDF rounding operator `sd - ρ`.
                // 0% = sharp vertices; 100% = the tip rounds all the way to a
                // disc of the same circumradius.
                PortDef::input("rounding", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Rounding")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:circle-notch")
                    .with_description("Round the corners: 0% = sharp, 100% = circle."),
                PortDef::input("softness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Softness")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:feather")
                    .with_description("Edge softness (0% = hard, 100% = feathered)"),
                // No `natural_range`: radians are a unit, not a normalized
                // signal — `pen.drawing_angle → rotation_input` passes through
                // raw and sums with the user's `rotation` offset.
                PortDef::input("rotation_input", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Rotation Input")
                    .with_unit(UnitType::Degrees)
                    .with_description(
                        "Live rotation, added on top of Rotation. Wire pen direction here so the shape follows your stroke.",
                    ),
                PortDef::input("rotation", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Rotation")
                    .with_unit(UnitType::Degrees)
                    // Orientation is part of shape identity; if the user
                    // exposes this knob, the dab thumbnail should follow it.
                    .persist_in_thumbnail()
                    .with_description(
                        "Spin the shape around its centre. Wire a changing signal into Rotation Input instead if you want it to move as you draw.",
                    ),
                PortDef::input("aspect", BrushWireType::Scalar)
                    .with_range(0.1, 1.0, 1.0)
                    .with_natural_range(0.1, 1.0)
                    .with_label("Aspect")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:pen-nib")
                    // Squash is part of shape identity, like rotation; the dab
                    // thumbnail should show the ellipse.
                    .persist_in_thumbnail()
                    .with_description(
                        "Squash the tip into an ellipse: 100% = round, lower = thinner. Rotates with the shape — set Rotation for a fixed-angle calligraphy nib.",
                    ),
                PortDef::output("mask", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .preview_image()
                    .with_description("Per-fragment mask value (0..1) — the polygon's alpha at this fragment"),
            ],
            is_gpu: true,
            is_terminal: false,
            supports_erase: true,
            preview_fallback_icon: None,
        },
    }
}

pub struct PolygonEvaluator;

impl BrushNodeEvaluator for PolygonEvaluator {
    /// Polygon coverage is per-fragment only — no CPU realisation, like the
    /// [`super::circle`] family.
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// Emit the SDF helper into `.decls` under a node-id-suffixed name (so two
    /// polygon nodes in one graph never redeclare a top-level fn — `.decls`
    /// are concatenated without dedup), and the per-fragment coverage into
    /// `.body`. Every input is either a literal (unwired) or an upstream
    /// expression (wired).
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("mask") {
            return Ok(wgsl);
        }

        let points = cctx.input("points").as_f32();
        let rounding = cctx.input("rounding").as_f32();
        let softness = cctx.input("softness").as_f32();
        let rotation = cctx.input("rotation").as_f32();
        let rotation_input = cctx.input("rotation_input").as_f32();
        let aspect = cctx.input("aspect").as_f32();

        // Signed distance to a regular n-gon of circumradius `r`, negative
        // inside, unit-gradient Euclidean field. Vertices sit on radius `r`.
        // Credit: Inigo Quilez, sdRegularPolygon
        // (https://iquilezles.org/articles/distfunctions2d/).
        let sdf = cctx.ident("polygon_sdf");
        wgsl.decls = format!(
            "fn {sdf}(p: vec2<f32>, n: f32, r: f32) -> f32 {{\n\
             \x20   let an = 3.14159265358979323846 / n;\n\
             \x20   let acs = vec2<f32>(cos(an), sin(an));\n\
             \x20   // Fold the point into one sector (angle measured from +y),\n\
             \x20   // reflecting about the bisector so distance reduces to\n\
             \x20   // distance-to-one-edge. `floor` gives a non-negative mod.\n\
             \x20   let a0 = atan2(p.x, p.y);\n\
             \x20   let two_an = 2.0 * an;\n\
             \x20   let bn = (a0 - two_an * floor(a0 / two_an)) - an;\n\
             \x20   var q = length(p) * vec2<f32>(cos(bn), abs(sin(bn)));\n\
             \x20   q = q - r * acs;\n\
             \x20   q.y = q.y + clamp(-q.y, 0.0, r * acs.y);\n\
             \x20   return length(q) * sign(q.x);\n\
             }}\n"
        );

        // φ folds in `view_rotation` so the tip is screen-relative like every
        // theta-based tip: the skeleton subtracts `view_rotation` from `theta`
        // (`wgsl/mod.rs`); working from raw `local_uv`, this node must apply
        // the same compensation itself or the polygon would spin with the
        // canvas view. Rotate `local_uv` by φ, then squash by `aspect`
        // (area-preserving, matching the circle family's ellipse). The
        // non-uniform aspect scale makes the field non-metric, so the softness
        // band is mildly anisotropic under squash — the same physics the
        // circle family's ellipse squash already lives with, not a new defect.
        let ident = cctx.ident("polygon");
        wgsl.body = format!(
            "    let {ident}_phi: f32 = -(({rotation}) + ({rotation_input}) + u.intrinsic.view_rotation);\n\
             \x20   let {ident}_c: f32 = cos({ident}_phi);\n\
             \x20   let {ident}_s: f32 = sin({ident}_phi);\n\
             \x20   let {ident}_rot: vec2<f32> = vec2<f32>(\n\
             \x20       local_uv.x * {ident}_c - local_uv.y * {ident}_s,\n\
             \x20       local_uv.x * {ident}_s + local_uv.y * {ident}_c,\n\
             \x20   );\n\
             \x20   let {ident}_aspect: f32 = clamp(({aspect}), 0.01, 1.0);\n\
             \x20   let {ident}_p: vec2<f32> = vec2<f32>({ident}_rot.x / {ident}_aspect, {ident}_rot.y * {ident}_aspect);\n\
             \x20   let {ident}_n: f32 = max(round(({points})), 3.0);\n\
             \x20   let {ident}_round: f32 = clamp(({rounding}), 0.0, 1.0);\n\
             \x20   // Circumradius `1 - ρ` plus SDF rounding `- ρ` keeps the\n\
             \x20   // rounded corners within a constant circumradius of 1.\n\
             \x20   let {ident}_sd: f32 = {sdf}({ident}_p, {ident}_n, 1.0 - {ident}_round) - {ident}_round;\n\
             \x20   // Feather *inward* from the boundary (`perp = -sd` is the\n\
             \x20   // signed distance inside), like the circle family's\n\
             \x20   // `shape_coverage`: coverage reaches 0 at the nominal edge and\n\
             \x20   // the soft band stays within the circumradius, so softness\n\
             \x20   // never blooms past the disc-clip and flattens into a circle.\n\
             \x20   let {ident}_band: f32 = max(clamp(({softness}), 0.0, 1.0), 0.004);\n\
             \x20   let {ident}: f32 = smoothstep(0.0, {ident}_band, -{ident}_sd);\n"
        );
        wgsl.outputs.insert("mask".into(), ident);
        Ok(wgsl)
    }

    /// The polygon's circumradius is a constant `1.0` (vertices on the unit
    /// circle; rounding stays within that circumradius), stretched by the
    /// worst-case anisotropy the `aspect` knob can deliver. Mirrors the
    /// circle family's aspect handling.
    fn extent(&self, ctx: &ExtentCtx) -> ExtentContribution {
        let aspect_min = ctx.port_min_value("aspect").max(0.01);
        let aspect_max = ctx.port_max_value("aspect").max(0.01);
        let aniso_max = (1.0 / aspect_min).max(aspect_max).max(1.0);
        ExtentContribution::Multiply(aniso_max)
    }
}
