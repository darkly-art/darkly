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
                // Amount of anisotropic squash. 0% = round; higher flattens the
                // tip into an ellipse. Paired with `squeeze_angle` (magnitude +
                // direction of the same squash).
                PortDef::input("squeeze", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Squeeze")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:pen-nib")
                    // Squash is part of shape identity, like rotation; the dab
                    // thumbnail should show the ellipse.
                    .persist_in_thumbnail()
                    .with_description(
                        "Squeeze the tip into an ellipse: 0% = round, higher = thinner. Aim it with Squeeze Angle.",
                    ),
                // Direction of the squeeze, independent of `rotation`: lets you
                // aim the nib's flat one way and the polygon's vertices another.
                // Measured relative to the (pre-rotation) shape frame, so it
                // co-rotates with the tip under canvas view rotation like every
                // other angle here.
                PortDef::input("squeeze_angle", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::PI, std::f32::consts::PI, 0.0)
                    .with_label("Squeeze Angle")
                    .with_unit(UnitType::Degrees)
                    .with_icon("fa6-solid:angles-left-right")
                    // Squeeze direction is part of shape identity, like rotation
                    // and squeeze; the dab thumbnail should follow it.
                    .persist_in_thumbnail()
                    .with_description(
                        "Direction of the Squeeze, independent of Rotation. Aim a calligraphy nib's flat separately from where the polygon's corners point.",
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
        let squeeze = cctx.input("squeeze").as_f32();
        let squeeze_angle = cctx.input("squeeze_angle").as_f32();

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

        // φ orients the polygon and folds in `view_rotation` so the tip is
        // screen-relative like every theta-based tip (the skeleton subtracts
        // `view_rotation` from `theta` in `wgsl/mod.rs`; working from raw
        // `local_uv`, this node re-applies that compensation itself, or the
        // polygon would spin with the canvas view).
        //
        // `squeeze` flattens the tip; `squeeze_angle` (β) aims that squash
        // independently of the polygon's rotation. The input transform rotates
        // `local_uv` into the squeeze-aligned frame (φ − β), scales x/y
        // (area-preserving), then rotates back by β — so the polygon sits at φ
        // while its squash axis sits at β from it. `squeeze` maps to a
        // semi-axis `a = 1 − 0.9·squeeze` (0% ⇒ round, 100% ⇒ a = 0.1).
        //
        // The squash is a non-uniform scale, so `sd_raw` measured in that space
        // is not a true screen distance — feeding it straight to the feather and
        // the rounding offset would stretch the softness band along the squash
        // axis. So it is divided by the squash's anisotropy factor
        // `|D·R(−β)·f| / |f|` (D = diag(1/a, a); f is the SDF gradient by central
        // difference — the same technique the circle family's `shape_coverage`
        // uses). That factor is exactly 1 when `a = 1`, so the un-squeezed tip is
        // untouched; under squash it rescales the band to a uniform screen-space
        // width on every edge. The zero-set (silhouette / extent) is untouched,
        // since dividing a signed distance by a positive scalar never moves its
        // sign change; the `max(…, 1e-3)` floors are divide-by-zero backstops.
        let ident = cctx.ident("polygon");
        wgsl.body = format!(
            "    let {ident}_beta: f32 = ({squeeze_angle});\n\
             \x20   let {ident}_phi: f32 = -(({rotation}) + ({rotation_input}) + u.intrinsic.view_rotation);\n\
             \x20   // Rotate local_uv into the squeeze-aligned frame (φ − β).\n\
             \x20   let {ident}_a1: f32 = {ident}_phi - {ident}_beta;\n\
             \x20   let {ident}_c: f32 = cos({ident}_a1);\n\
             \x20   let {ident}_s: f32 = sin({ident}_a1);\n\
             \x20   let {ident}_rot: vec2<f32> = vec2<f32>(\n\
             \x20       local_uv.x * {ident}_c - local_uv.y * {ident}_s,\n\
             \x20       local_uv.x * {ident}_s + local_uv.y * {ident}_c,\n\
             \x20   );\n\
             \x20   // Semi-axis from squeeze (0% ⇒ round, 100% ⇒ 0.1); squash x/y.\n\
             \x20   let {ident}_a: f32 = clamp(1.0 - 0.9 * clamp(({squeeze}), 0.0, 1.0), 0.01, 1.0);\n\
             \x20   let {ident}_sq: vec2<f32> = vec2<f32>({ident}_rot.x / {ident}_a, {ident}_rot.y * {ident}_a);\n\
             \x20   // Rotate back by β so the polygon sits at φ, its squash axis at β.\n\
             \x20   let {ident}_cb: f32 = cos({ident}_beta);\n\
             \x20   let {ident}_sb: f32 = sin({ident}_beta);\n\
             \x20   let {ident}_p: vec2<f32> = vec2<f32>(\n\
             \x20       {ident}_sq.x * {ident}_cb - {ident}_sq.y * {ident}_sb,\n\
             \x20       {ident}_sq.x * {ident}_sb + {ident}_sq.y * {ident}_cb,\n\
             \x20   );\n\
             \x20   let {ident}_n: f32 = max(round(({points})), 3.0);\n\
             \x20   let {ident}_round: f32 = clamp(({rounding}), 0.0, 1.0);\n\
             \x20   // Circumradius `1 - ρ`: the base polygon shrinks so the fillet\n\
             \x20   // (added after normalization) keeps the tip within circumradius 1.\n\
             \x20   let {ident}_cr: f32 = 1.0 - {ident}_round;\n\
             \x20   let {ident}_sd_raw: f32 = {sdf}({ident}_p, {ident}_n, {ident}_cr);\n\
             \x20   // Central-difference gradient of the SDF in the squashed p-space.\n\
             \x20   let {ident}_h: f32 = 1e-3;\n\
             \x20   let {ident}_gx: f32 = {sdf}({ident}_p + vec2<f32>({ident}_h, 0.0), {ident}_n, {ident}_cr)\n\
             \x20                       - {sdf}({ident}_p - vec2<f32>({ident}_h, 0.0), {ident}_n, {ident}_cr);\n\
             \x20   let {ident}_gy: f32 = {sdf}({ident}_p + vec2<f32>(0.0, {ident}_h), {ident}_n, {ident}_cr)\n\
             \x20                       - {sdf}({ident}_p - vec2<f32>(0.0, {ident}_h), {ident}_n, {ident}_cr);\n\
             \x20   let {ident}_f: vec2<f32> = vec2<f32>({ident}_gx, {ident}_gy) / (2.0 * {ident}_h);\n\
             \x20   // Anisotropy factor |D·R(−β)·f| / |f|: how much the squeeze\n\
             \x20   // stretches the SDF gradient. Exactly 1 when a = 1 (un-squeezed);\n\
             \x20   // dividing by it rescales the band to a uniform screen-space width.\n\
             \x20   let {ident}_fr: vec2<f32> = vec2<f32>(\n\
             \x20       {ident}_f.x * {ident}_cb + {ident}_f.y * {ident}_sb,\n\
             \x20       -{ident}_f.x * {ident}_sb + {ident}_f.y * {ident}_cb,\n\
             \x20   );\n\
             \x20   let {ident}_df: f32 = length(vec2<f32>({ident}_fr.x / {ident}_a, {ident}_fr.y * {ident}_a));\n\
             \x20   let {ident}_g: f32 = max({ident}_df / max(length({ident}_f), 1e-3), 1e-4);\n\
             \x20   // True screen-space signed distance, then the corner fillet.\n\
             \x20   // Feather *inward* from the boundary like the circle family's\n\
             \x20   // `shape_coverage`: coverage reaches 0 at the nominal edge, so\n\
             \x20   // softness never blooms past the disc-clip and flattens the tip.\n\
             \x20   let {ident}_sd: f32 = {ident}_sd_raw / {ident}_g - {ident}_round;\n\
             \x20   let {ident}_band: f32 = max(clamp(({softness}), 0.0, 1.0), 0.004);\n\
             \x20   let {ident}: f32 = smoothstep(0.0, {ident}_band, -{ident}_sd);\n"
        );
        wgsl.outputs.insert("mask".into(), ident);
        Ok(wgsl)
    }

    /// The polygon's circumradius is a constant `1.0` (vertices on the unit
    /// circle; rounding stays within that circumradius), stretched by the
    /// worst-case anisotropy the `squeeze` knob can deliver — the tip grows by
    /// `1/a` along the stretched axis, where `a = 1 − 0.9·squeeze` is the
    /// semi-axis (matching the emitted body).
    fn extent(&self, ctx: &ExtentCtx) -> ExtentContribution {
        let squeeze_max = ctx.port_max_value("squeeze").clamp(0.0, 1.0);
        let a_min = (1.0 - 0.9 * squeeze_max).max(0.01);
        let aniso_max = (1.0 / a_min).max(1.0);
        ExtentContribution::Multiply(aniso_max)
    }
}
