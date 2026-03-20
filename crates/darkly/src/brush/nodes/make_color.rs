//! Make Color node — (R, G, B, A) → Color.
//!
//! Constructs a Color value from individual scalar components.

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "make_color",
        category: "color",
        display_name: "Make Color",
        ports: vec![
            PortDef::input("r", BrushWireType::Scalar).with_range(0.0, 1.0, 0.0),
            PortDef::input("g", BrushWireType::Scalar).with_range(0.0, 1.0, 0.0),
            PortDef::input("b", BrushWireType::Scalar).with_range(0.0, 1.0, 0.0),
            PortDef::input("a", BrushWireType::Scalar).with_range(0.0, 1.0, 1.0),
            PortDef::output("color", BrushWireType::Color),
        ],
        params: &[],
        is_gpu: false,
    }
}

pub struct MakeColorEvaluator;

impl BrushNodeEvaluator for MakeColorEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let r = ctx.input_f32("r");
        let g = ctx.input_f32("g");
        let b = ctx.input_f32("b");
        let a = ctx.input_f32("a");
        vec![("color".into(), ScalarValue::Color([r, g, b, a]))]
    }
}
