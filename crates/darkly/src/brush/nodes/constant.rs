//! Constant node — outputs a fixed scalar value from a parameter.

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "constant",
        category: "math",
        display_name: "Constant",
        ports: vec![
            PortDef::output("value", BrushWireType::Scalar),
        ],
        params: &[
            ParamDef::Float { name: "value", min: 0.0, max: 1.0, default: 0.5 },
        ],
        is_gpu: false,
    }
}

pub struct ConstantEvaluator;

impl BrushNodeEvaluator for ConstantEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let value = ctx.param_f32(0);
        vec![("value".into(), ScalarValue::Scalar(value))]
    }
}
