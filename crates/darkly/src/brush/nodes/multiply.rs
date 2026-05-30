//! Multiply node — Scalar * Scalar → Scalar.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "multiply";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "math",
            display_name: "Multiply",
            ports: vec![
                PortDef::input("a", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_description("First factor"),
                PortDef::input("b", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_description("Second factor"),
                PortDef::output("result", BrushWireType::Scalar)
                    .with_description("Product of a \u{00d7} b"),
            ],
            params: &[],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
        },
        || Box::new(MultiplyEvaluator),
    )
}

pub struct MultiplyEvaluator;

impl BrushNodeEvaluator for MultiplyEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let a = ctx.input_f32("a");
        let b = ctx.input_f32("b");
        vec![("result".into(), ScalarValue::Scalar(a * b))]
    }

    /// Inline scalar product into the compiled fragment shader.
    /// Required so brushes that route scalars through `multiply` on
    /// the way to the `paint` terminal (e.g. Charcoal:
    /// `paper.luminance * threshold * circle.coverage`) compile —
    /// every upstream node of a compiled terminal must emit WGSL.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("result") {
            return Ok(wgsl);
        }
        let a = cctx.input("a").as_f32();
        let b = cctx.input("b").as_f32();
        wgsl.outputs
            .insert("result".into(), format!("(({a}) * ({b}))"));
        Ok(wgsl)
    }
}
