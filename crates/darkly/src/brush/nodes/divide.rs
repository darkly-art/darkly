//! Divide node: Scalar / Scalar → Scalar.

use crate::brush::eval::BrushNodeEvaluator;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::scalar_binary::{ScalarBinaryNode, ScalarBinaryOp};

pub const TYPE_ID: &str = "divide";

pub fn register() -> BrushNodeRegistration {
    ScalarBinaryNode {
        type_id: TYPE_ID,
        display_name: "Divide",
        description: "Divides one value by another: use it to normalize one signal against another, e.g. scale pressure down where a texture is bright. Dividing by zero yields zero.",
        a_description: "Dividend",
        b_description: "Divisor",
        result_description: "Quotient of a \u{00f7} b (zero when b is zero)",
        identity: 1.0,
        evaluator,
    }
    .register()
}

fn evaluator() -> Box<dyn BrushNodeEvaluator> {
    Box::new(ScalarBinaryOp {
        apply: |a, b| if b == 0.0 { 0.0 } else { a / b },
        wgsl: |a, b| format!("select(({a}) / ({b}), 0.0, ({b}) == 0.0)"),
    })
}
