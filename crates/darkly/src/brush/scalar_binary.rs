//! Shared implementation for scalar binary-math nodes (multiply, add,
//! subtract). Each is `Scalar op Scalar → Scalar`, differing only in the
//! operator and its identity element; this factors the evaluator, the WGSL
//! emission, and the port layout so a new operator is one small node file
//! wiring up a [`ScalarBinaryOp`] — no consumer edits.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

/// A scalar binary math operator: `result = op(a, b)`.
///
/// `apply` evaluates the operator on the CPU; `wgsl_op` is the infix token
/// (`*`, `+`, `-`) inlined into compiled fragment shaders so brushes that
/// route scalars through this node on the way to a compiled terminal still
/// emit WGSL — every upstream node of a compiled terminal must.
pub struct ScalarBinaryOp {
    pub apply: fn(f32, f32) -> f32,
    pub wgsl_op: &'static str,
}

impl BrushNodeEvaluator for ScalarBinaryOp {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let a = ctx.input_f32("a");
        let b = ctx.input_f32("b");
        vec![("result".into(), ScalarValue::Scalar((self.apply)(a, b)))]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("result") {
            return Ok(wgsl);
        }
        let a = cctx.input("a").as_f32();
        let b = cctx.input("b").as_f32();
        let op = self.wgsl_op;
        wgsl.outputs
            .insert("result".into(), format!("(({a}) {op} ({b}))"));
        Ok(wgsl)
    }
}

/// Static description of one scalar binary-math node — everything that varies
/// between multiply/add/subtract. `identity` is the operator's identity
/// element, used as both inputs' default so an unconnected port is a no-op.
pub struct ScalarBinaryNode {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub a_description: &'static str,
    pub b_description: &'static str,
    pub result_description: &'static str,
    pub identity: f32,
    pub evaluator: fn() -> Box<dyn BrushNodeEvaluator>,
}

impl ScalarBinaryNode {
    pub fn register(&self) -> BrushNodeRegistration {
        BrushNodeRegistration::compute(
            NodeRegistration {
                type_id: self.type_id,
                category: "math",
                display_name: self.display_name,
                description: self.description,
                ports: vec![
                    PortDef::input("a", BrushWireType::Scalar)
                        .with_range(0.0, 1.0, self.identity)
                        .with_description(self.a_description),
                    PortDef::input("b", BrushWireType::Scalar)
                        .with_range(0.0, 1.0, self.identity)
                        .with_description(self.b_description),
                    PortDef::output("result", BrushWireType::Scalar)
                        .with_description(self.result_description),
                ],
                is_gpu: false,
                is_terminal: false,
                supports_erase: true,
                preview_fallback_icon: None,
            },
            self.evaluator,
        )
    }
}
