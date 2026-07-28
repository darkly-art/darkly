//! Subtract node — Scalar - Scalar → Scalar.

use crate::brush::eval::BrushNodeEvaluator;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::scalar_binary::{ScalarBinaryNode, ScalarBinaryOp};

pub const TYPE_ID: &str = "subtract";

pub fn register() -> BrushNodeRegistration {
    ScalarBinaryNode {
        type_id: TYPE_ID,
        display_name: "Subtract",
        description: "Subtracts one value from another — use it to attenuate one signal by another, e.g. reduce pressure where a texture is bright.",
        a_description: "Minuend",
        b_description: "Subtrahend",
        result_description: "Difference of a - b",
        identity: 0.0,
        evaluator,
    }
    .register()
}

fn evaluator() -> Box<dyn BrushNodeEvaluator> {
    Box::new(ScalarBinaryOp {
        apply: |a, b| a - b,
        wgsl_op: "-",
    })
}
