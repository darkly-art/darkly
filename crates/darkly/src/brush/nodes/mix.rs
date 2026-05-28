//! Mix (linear interpolation) node — mix(a, b, t) → Scalar or Color.
//!
//! Lerps between two values based on a factor.  Works for both Scalar
//! and Color inputs (Color inputs lerp component-wise).

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "mix";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(NodeRegistration {
        type_id: TYPE_ID,
        category: "math",
        display_name: "Mix",
        ports: vec![
            PortDef::input("a", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_description("Value at factor = 0"),
            PortDef::input("b", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0)
                .with_description("Value at factor = 1"),
            PortDef::input("factor", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_natural_range(0.0, 1.0)
                .with_description("Blend factor (0 = all a, 1 = all b)"),
            PortDef::output("result", BrushWireType::Scalar)
                .with_description("Interpolated scalar result"),
            PortDef::output("color_result", BrushWireType::Color)
                .with_description("Interpolated color result (when color inputs are wired)"),
        ],
        params: &[],
        is_gpu: false,
        is_terminal: false,
        supports_erase: true,
    })
}

pub struct MixEvaluator;

impl BrushNodeEvaluator for MixEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let t = ctx.input_f32("factor");

        // Scalar lerp.
        let a_f = ctx.input_f32("a");
        let b_f = ctx.input_f32("b");
        let scalar_result = a_f + (b_f - a_f) * t;

        // Color lerp (works when Color inputs are wired).
        let a_c = ctx.input("a").as_color();
        let b_c = ctx.input("b").as_color();
        let color_result = [
            a_c[0] + (b_c[0] - a_c[0]) * t,
            a_c[1] + (b_c[1] - a_c[1]) * t,
            a_c[2] + (b_c[2] - a_c[2]) * t,
            a_c[3] + (b_c[3] - a_c[3]) * t,
        ];

        vec![
            ("result".into(), ScalarValue::Scalar(scalar_result)),
            ("color_result".into(), ScalarValue::Color(color_result)),
        ]
    }
}
