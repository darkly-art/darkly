//! Clamp node — clamp(Scalar, min, max) → Scalar.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "clamp";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(NodeRegistration {
        type_id: TYPE_ID,
        category: "math",
        display_name: "Clamp",
        ports: vec![
            PortDef::input("value", BrushWireType::Scalar).with_description("Input value to clamp"),
            PortDef::output("result", BrushWireType::Scalar)
                .with_description("Clamped output value"),
        ],
        params: &[
            ParamDef::Float {
                name: "min",
                min: 0.0,
                max: 1.0,
                default: 0.0,
            },
            ParamDef::Float {
                name: "max",
                min: 0.0,
                max: 1.0,
                default: 1.0,
            },
        ],
        is_gpu: false,
        is_terminal: false,
        supports_erase: true,
    })
}

pub struct ClampEvaluator;

impl BrushNodeEvaluator for ClampEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let value = ctx.input_f32("value");
        let min = ctx.param_f32(0);
        let max = ctx.param_f32(1);
        vec![("result".into(), ScalarValue::Scalar(value.clamp(min, max)))]
    }
}
