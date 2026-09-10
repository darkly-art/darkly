//! Multiply node: Scalar * Scalar → Scalar.

use crate::brush::eval::BrushNodeEvaluator;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::scalar_binary::{ScalarBinaryNode, ScalarBinaryOp};

pub const TYPE_ID: &str = "multiply";

pub fn register() -> BrushNodeRegistration {
    ScalarBinaryNode {
        type_id: TYPE_ID,
        display_name: "Multiply",
        description: "Multiplies two values: use it to scale one signal by another, e.g. fade pressure by texture.",
        a_description: "First factor",
        b_description: "Second factor",
        result_description: "Product of a \u{00d7} b",
        identity: 1.0,
        evaluator,
    }
    .register()
}

fn evaluator() -> Box<dyn BrushNodeEvaluator> {
    Box::new(ScalarBinaryOp {
        apply: |a, b| a * b,
        wgsl: |a, b| format!("(({a}) * ({b}))"),
    })
}
