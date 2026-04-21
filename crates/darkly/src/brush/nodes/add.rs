//! Add node — Scalar + Scalar → Scalar.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "add",
        category: "math",
        display_name: "Add",
        ports: vec![
            PortDef::input("a", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_description("First addend"),
            PortDef::input("b", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_description("Second addend"),
            PortDef::output("result", BrushWireType::Scalar).with_description("Sum of a + b"),
        ],
        params: &[],
        is_gpu: false,
    }
}

pub struct AddEvaluator;

impl BrushNodeEvaluator for AddEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let a = ctx.input_f32("a");
        let b = ctx.input_f32("b");
        vec![("result".into(), ScalarValue::Scalar(a + b))]
    }
}
