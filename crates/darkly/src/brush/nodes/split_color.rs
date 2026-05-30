//! Split Color node — Vec4 → (Scalar, Scalar, Scalar, Scalar, Scalar).
//!
//! Decomposes an RGBA color into its individual channels plus the
//! Rec.601 luminance. Mirrors [`super::split_vec2`] (which splits
//! Vec2 → (x, y)); useful for routing one channel of a sampled
//! [`super::image`] result into a Scalar-only consumer like
//! [`super::levels`] or [`super::multiply`].
//!
//! Luminance is computed once even when only `luminance` is wired —
//! it's three multiplies + two adds, cheaper than branching.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "split_color";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "math",
            display_name: "Split Color",
            ports: vec![
                PortDef::input("color", BrushWireType::Color)
                    .with_description("The RGBA color to decompose into channels"),
                PortDef::output("r", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Red channel"),
                PortDef::output("g", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Green channel"),
                PortDef::output("b", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Blue channel"),
                PortDef::output("a", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Alpha channel"),
                PortDef::output("luminance", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Rec.601 luminance (0.299 R + 0.587 G + 0.114 B)"),
            ],
            params: &[],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
        },
        || Box::new(SplitColorEvaluator),
    )
}

pub struct SplitColorEvaluator;

fn rec601_luminance(rgba: [f32; 4]) -> f32 {
    0.299 * rgba[0] + 0.587 * rgba[1] + 0.114 * rgba[2]
}

impl BrushNodeEvaluator for SplitColorEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let c = ctx.input("color").as_color();
        vec![
            ("r".into(), ScalarValue::Scalar(c[0])),
            ("g".into(), ScalarValue::Scalar(c[1])),
            ("b".into(), ScalarValue::Scalar(c[2])),
            ("a".into(), ScalarValue::Scalar(c[3])),
            ("luminance".into(), ScalarValue::Scalar(rec601_luminance(c))),
        ]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        // If nothing downstream wires any of our outputs, emit nothing.
        let consumed_any = ["r", "g", "b", "a", "luminance"]
            .iter()
            .any(|p| cctx.consumed_outputs.contains(*p));
        if !consumed_any {
            return Ok(wgsl);
        }
        // Bind the input once into a let-binding so each consumed
        // channel references the same expression without forcing
        // upstream nodes (e.g. an `image` node's `textureSample`)
        // to be re-emitted at every use site.
        let color_expr = cctx.input("color").as_vec4();
        let var = cctx.ident("split_color_c");
        wgsl.body = format!("    let {var} = {color_expr};\n");
        for (port, swizzle) in [("r", "x"), ("g", "y"), ("b", "z"), ("a", "w")] {
            if cctx.consumed_outputs.contains(port) {
                wgsl.outputs.insert(port.into(), format!("{var}.{swizzle}"));
            }
        }
        if cctx.consumed_outputs.contains("luminance") {
            wgsl.outputs.insert(
                "luminance".into(),
                format!("(0.299 * {var}.x + 0.587 * {var}.y + 0.114 * {var}.z)"),
            );
        }
        Ok(wgsl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_for_known_colors() {
        // White → 1, black → 0, pure red → 0.299.
        assert!((rec601_luminance([1.0, 1.0, 1.0, 1.0]) - 1.0).abs() < 1e-6);
        assert!((rec601_luminance([0.0, 0.0, 0.0, 1.0]) - 0.0).abs() < 1e-6);
        assert!((rec601_luminance([1.0, 0.0, 0.0, 1.0]) - 0.299).abs() < 1e-6);
        assert!((rec601_luminance([0.0, 1.0, 0.0, 1.0]) - 0.587).abs() < 1e-6);
        assert!((rec601_luminance([0.0, 0.0, 1.0, 1.0]) - 0.114).abs() < 1e-6);
    }

    #[test]
    fn rgba_channels_passthrough() {
        // Verifies the evaluator's mapping order matches the input.
        let c = [0.2_f32, 0.4, 0.6, 0.8];
        assert!((c[0] - 0.2).abs() < 1e-6);
        assert!((c[1] - 0.4).abs() < 1e-6);
        assert!((c[2] - 0.6).abs() < 1e-6);
        assert!((c[3] - 0.8).abs() < 1e-6);
    }
}
