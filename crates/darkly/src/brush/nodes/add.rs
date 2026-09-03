//! Add node: Scalar + Scalar → Scalar.

use crate::brush::eval::BrushNodeEvaluator;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::scalar_binary::{ScalarBinaryNode, ScalarBinaryOp};

pub const TYPE_ID: &str = "add";

pub fn register() -> BrushNodeRegistration {
    ScalarBinaryNode {
        type_id: TYPE_ID,
        display_name: "Add",
        description: "Adds two values; use it to offset one signal by another, e.g. lift pressure by a constant floor.",
        a_description: "First addend",
        b_description: "Second addend",
        result_description: "Sum of a + b",
        identity: 0.0,
        evaluator,
    }
    .register()
}

fn evaluator() -> Box<dyn BrushNodeEvaluator> {
    Box::new(ScalarBinaryOp {
        apply: |a, b| a + b,
        wgsl: |a, b| format!("(({a}) + ({b}))"),
    })
}
