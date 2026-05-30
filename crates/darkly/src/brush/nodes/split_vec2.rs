//! Split Vec2 node — Vec2 → (Scalar, Scalar).
//!
//! Decomposes a two-component vector into its X and Y components.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "split_vec2";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "math",
            display_name: "Split Vec2",
            ports: vec![
                PortDef::input("vec", BrushWireType::Vec2)
                    .with_description("The 2D vector to split into components"),
                PortDef::output("x", BrushWireType::Scalar)
                    .with_description("Horizontal (X) component of the vector"),
                PortDef::output("y", BrushWireType::Scalar)
                    .with_description("Vertical (Y) component of the vector"),
            ],
            params: &[],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
        },
        || Box::new(SplitVec2Evaluator),
    )
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
