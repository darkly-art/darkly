//! Procedural regular-polygon coverage GPU node — a straight-edged brush tip.
//!
//! Compile-only: contributes a per-fragment scalar coverage expression
//! (`f32` in `[0, 1]`) to the brush's compiled WGSL via [`compile_wgsl`],
//! inlined by downstream consumers (`stamp.tip`, …) exactly like
//! [`super::circle`]'s `mask` — no dab texture, no separate pass.
//!
//! Unlike the [`super::circle`] family, which measures coverage from a
//! polar radius `r(θ)` corrected by a per-angle gradient, polygon coverage
//! is a **true signed-distance field** — the exact screen distance to the
//! (squeezed) polygon. That is what makes the softness band a uniform width
//! and lets `squeeze` and rounding compose cleanly: an exact SDF has parallel
//! iso-contours everywhere, so the corner fillet is a real circular arc
//! tangent to the edges rather than a stretched, ballooning approximation.
//!
//! Credit: the per-edge min-distance + winding-sign polygon SDF is Inigo
//! Quilez's "distance to a polygon" (`sdPoly`),
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
        scratch_format: crate::brush::node::COLOR_SCRATCH_FORMAT,
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
                    .with_range(0.0, 1.0, 0.0)
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
            preview_staging: None,
        },
    }
}

/// Support of the rounded, squeezed silhouette, in units of the dab radius —
/// the largest distance from the dab centre at which [`compile_wgsl`] can
/// produce non-zero coverage.
///
/// `a` is the squeeze semi-axis (`1 − 0.9·squeeze`), `rounding` the corner
/// radius `ρ`, `n` the side count, and `beta` the squeeze angle — or `None`
/// when the axis is not known at compile time, which yields the
/// orientation-agnostic worst case of a vertex on the stretched axis.
///
/// Shared by [`PolygonEvaluator::extent`] and the feature test that asserts
/// nothing lies outside the bound, so the budgeted extent and the silhouette
/// it bounds cannot drift apart.
///
/// [`compile_wgsl`]: PolygonEvaluator::compile_wgsl
pub fn silhouette_support(a: f32, rounding: f32, n: f32, beta: Option<f32>) -> f32 {
    let a = a.max(0.01);
    let rounding = rounding.clamp(0.0, 1.0);
    // The body builds the polygon at circumradius `1 − ρ` and then dilates the
    // distance field by `ρ`, so those two are the whole reach.
    let cr = 1.0 - rounding;
    let radial = match beta {
        None => 1.0 / a,
        Some(beta) => {
            let n = n.round().max(3.0);
            let (sb, cb) = beta.sin_cos();
            (0..n as u32).fold(0.0_f32, |acc, i| {
                // Vertex i's base direction, matching the emitted body's
                // `vec2(sin(ak), cos(ak))`, carried into the squeeze frame by
                // `R(−β)` and scaled by `diag(a, 1/a)`.
                let (s, c) = (std::f32::consts::TAU * i as f32 / n).sin_cos();
                let wx = s * cb + c * sb;
                let wy = c * cb - s * sb;
                acc.max(((a * wx).powi(2) + (wy / a).powi(2)).sqrt())
            })
        }
    };
    cr * radial + rounding
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

        // Exact signed distance to the squeezed regular n-gon, in screen space:
        // negative inside, unit-gradient Euclidean field. `tinv` maps a base
        // circumradius-`r` vertex (regular, vertex on +y) into screen space, so
        // the polygon is generated already squeezed and oriented — the distance
        // is then a *true* screen distance, which is what makes the softness band
        // uniform and lets the rounding operator carve real circular fillets.
        // Credit: Inigo Quilez, "distance to a polygon" (sdPoly, the per-edge
        // min-distance + winding-sign form), https://iquilezles.org/articles/distfunctions2d/.
        let sdf = cctx.ident("polygon_sdf");
        wgsl.decls = format!(
            "fn {sdf}(uv: vec2<f32>, tinv: mat2x2<f32>, n: f32, r: f32) -> f32 {{\n\
             \x20   let ni = i32(n);\n\
             \x20   let two_pi = 6.28318530717958647692;\n\
             \x20   // Previous vertex (k = n-1), in screen space.\n\
             \x20   let ak0 = two_pi * f32(ni - 1) / n;\n\
             \x20   var vj = tinv * (r * vec2<f32>(sin(ak0), cos(ak0)));\n\
             \x20   var d2 = 1e18;\n\
             \x20   var s = 1.0;\n\
             \x20   for (var i = 0; i < ni; i = i + 1) {{\n\
             \x20       let ak = two_pi * f32(i) / n;\n\
             \x20       let vi = tinv * (r * vec2<f32>(sin(ak), cos(ak)));\n\
             \x20       // Distance to edge (vi → vj), clamped to the segment.\n\
             \x20       let e = vj - vi;\n\
             \x20       let w = uv - vi;\n\
             \x20       let b = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-12), 0.0, 1.0);\n\
             \x20       d2 = min(d2, dot(b, b));\n\
             \x20       // Winding: flip sign on each edge the +x ray crosses.\n\
             \x20       let c0 = uv.y >= vi.y;\n\
             \x20       let c1 = uv.y < vj.y;\n\
             \x20       let c2 = (e.x * w.y) > (e.y * w.x);\n\
             \x20       if ((c0 && c1 && c2) || (!c0 && !c1 && !c2)) {{ s = -s; }}\n\
             \x20       vj = vi;\n\
             \x20   }}\n\
             \x20   return s * sqrt(d2);\n\
             }}\n"
        );

        // φ orients the polygon and folds in `view_rotation` so the tip is
        // screen-relative like every theta-based tip (the skeleton subtracts
        // `view_rotation` from `theta` in `wgsl/mod.rs`; working from raw
        // `local_uv`, this node re-applies that compensation itself, or the
        // polygon would spin with the canvas view).
        //
        // `squeeze` flattens the tip; `squeeze_angle` (β) aims that squash
        // independently of the polygon's rotation. Rather than squash the query
        // point and measure distance in the distorted space (which stretches the
        // band and the corner fillets), the polygon's vertices are generated
        // *already squeezed and oriented* in screen space via the inverse
        // transform `T⁻¹ = R(β−φ)·diag(a,1/a)·R(−β)`, and the SDF returns a true
        // screen distance. `squeeze` maps to a semi-axis `a = 1 − 0.9·squeeze`
        // (0% ⇒ round, 100% ⇒ a = 0.1).
        //
        // Because the distance is exact and Euclidean, the softness band is a
        // uniform width on every edge under any squeeze, and the rounding
        // operator `sd − ρ` carves true circular corner fillets tangent to the
        // edges — a squeezed square rounds into a proper rounded rectangle
        // (stadium ends when ρ exceeds the short half-side), never a balloon. The
        // base polygon is generated at circumradius `1 − ρ` so the ρ-radius
        // fillet keeps the tip within its nominal footprint (extent bound).
        let ident = cctx.ident("polygon");
        wgsl.body = format!(
            "    let {ident}_beta: f32 = ({squeeze_angle});\n\
             \x20   let {ident}_n: f32 = max(round(({points})), 3.0);\n\
             \x20   // Base orientation: half the angular step (π/n) rotates the\n\
             \x20   // vertex-up polygon so a flat edge sits horizontal — the natural\n\
             \x20   // \"resting on a base\" look. Reduces to +45° for a square (n=4),\n\
             \x20   // 60° for a triangle, 30° for a hexagon. Rotation inputs and view\n\
             \x20   // compensation add on top.\n\
             \x20   let {ident}_phi: f32 = -(3.14159265358979323846 / {ident}_n + ({rotation}) + ({rotation_input}) + u.intrinsic.view_rotation);\n\
             \x20   // Semi-axis from squeeze (0% ⇒ round, 100% ⇒ 0.1).\n\
             \x20   let {ident}_a: f32 = clamp(1.0 - 0.9 * clamp(({squeeze}), 0.0, 1.0), 0.01, 1.0);\n\
             \x20   // Inverse squeeze transform: screen = R(β−φ)·diag(a,1/a)·R(−β)·p,\n\
             \x20   // used to place each base vertex already squeezed and oriented.\n\
             \x20   let {ident}_cb: f32 = cos({ident}_beta);\n\
             \x20   let {ident}_sb: f32 = sin({ident}_beta);\n\
             \x20   let {ident}_cbp: f32 = cos({ident}_beta - {ident}_phi);\n\
             \x20   let {ident}_sbp: f32 = sin({ident}_beta - {ident}_phi);\n\
             \x20   let {ident}_tinv: mat2x2<f32> =\n\
             \x20       mat2x2<f32>({ident}_cbp, {ident}_sbp, -{ident}_sbp, {ident}_cbp)\n\
             \x20       * mat2x2<f32>({ident}_a, 0.0, 0.0, 1.0 / {ident}_a)\n\
             \x20       * mat2x2<f32>({ident}_cb, -{ident}_sb, {ident}_sb, {ident}_cb);\n\
             \x20   let {ident}_round: f32 = clamp(({rounding}), 0.0, 1.0);\n\
             \x20   let {ident}_cr: f32 = 1.0 - {ident}_round;\n\
             \x20   // Exact screen distance to the squeezed polygon, then a real\n\
             \x20   // circular corner fillet. Feather *inward* from the boundary\n\
             \x20   // (coverage reaches 0 at the edge), like the circle family's\n\
             \x20   // `shape_coverage`, so softness never blooms past the disc-clip.\n\
             \x20   let {ident}_sd: f32 = {sdf}(local_uv, {ident}_tinv, {ident}_n, {ident}_cr) - {ident}_round;\n\
             \x20   let {ident}_band: f32 = max(clamp(({softness}), 0.0, 1.0), 0.004);\n\
             \x20   let {ident}: f32 = smoothstep(0.0, {ident}_band, -{ident}_sd);\n"
        );
        wgsl.outputs.insert("mask".into(), ident);
        Ok(wgsl)
    }

    /// Support of the silhouette the emitted body actually paints, in units of
    /// the dab radius.
    ///
    /// `compile_wgsl` builds the polygon at circumradius `cr = 1 − ρ`, maps its
    /// vertices through `T⁻¹` (semi-axes `a` and `1/a`), and then dilates the
    /// result by the rounding radius `ρ` — `sd − ρ` is an *isotropic* offset
    /// applied after the anisotropic map. So the reach is
    ///
    /// ```text
    /// cr · maxᵢ ‖ diag(a, 1/a) · R(−β) · v̂ᵢ ‖ + ρ
    /// ```
    ///
    /// Bounding that with the ellipse's semi-major `1/a` is correct but loose:
    /// it assumes `ρ = 0` *and* that some vertex lands on the stretched axis.
    /// Looseness is not free here — the fragment stage's only early-out is a
    /// circular discard at this radius, so every pixel inside the bound is
    /// fully shaded (SDF loop included) before its coverage is evaluated.
    ///
    /// Only each mapped vertex's *magnitude* matters, and `T⁻¹`'s outer
    /// `R(β − φ)` is a rotation, which preserves magnitude. Per-dab spin —
    /// `rotation_input` (Sponge wires pen direction into it) and
    /// `view_rotation` — therefore cannot affect this bound, which is what
    /// makes evaluating it once at compile time sound.
    fn extent(&self, ctx: &ExtentCtx) -> ExtentContribution {
        let squeeze_max = ctx.port_max_value("squeeze").clamp(0.0, 1.0);
        let a = 1.0 - 0.9 * squeeze_max;
        // `β` and `n` decide which vertices sit where relative to the stretched
        // axis. A wired input's value is unknown here, so drop to the
        // orientation-agnostic worst case rather than guessing an axis.
        let axis_known =
            !ctx.wired_inputs.contains("squeeze_angle") && !ctx.wired_inputs.contains("points");
        let beta = axis_known.then(|| ctx.port_max_value("squeeze_angle"));
        ExtentContribution::Multiply(silhouette_support(
            a,
            ctx.port_max_value("rounding"),
            ctx.port_max_value("points"),
            beta,
        ))
    }
}
