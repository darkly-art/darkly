//! Multiply node — Scalar * Scalar → Scalar.

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "multiply",
        category: "math",
        display_name: "Multiply",
        ports: vec![
            PortDef::input("a", BrushWireType::Scalar).with_range(0.0, 1.0, 1.0),
            PortDef::input("b", BrushWireType::Scalar).with_range(0.0, 1.0, 1.0),
            PortDef::output("result", BrushWireType::Scalar),
        ],
        params: &[],
        is_gpu: false,
    }
}

pub struct MultiplyEvaluator;

impl BrushNodeEvaluator for MultiplyEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let a = ctx.input_f32("a");
        let b = ctx.input_f32("b");
        vec![("result".into(), ScalarValue::Scalar(a * b))]
    }
}
