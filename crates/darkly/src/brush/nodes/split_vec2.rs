//! Split Vec2 node — Vec2 → (Scalar, Scalar).
//!
//! Decomposes a two-component vector into its X and Y components.

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "split_vec2",
        category: "math",
        display_name: "Split Vec2",
        ports: vec![
            PortDef::input("vec", BrushWireType::Vec2),
            PortDef::output("x", BrushWireType::Scalar),
            PortDef::output("y", BrushWireType::Scalar),
        ],
        params: &[],
        is_gpu: false,
    }
}

pub struct SplitVec2Evaluator;

impl BrushNodeEvaluator for SplitVec2Evaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let v = ctx.input("vec").as_vec2();
        vec![
            ("x".into(), ScalarValue::Scalar(v[0])),
            ("y".into(), ScalarValue::Scalar(v[1])),
        ]
    }
}
