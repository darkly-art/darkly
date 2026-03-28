//! Remap node — remap(Scalar, in_range, out_range) → Scalar.
//!
//! Maps an input value from [in_min, in_max] to [out_min, out_max].
//! Useful for restricting a sensor to a sub-range (e.g. only the top
//! half of pressure affecting size).

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "remap",
        category: "math",
        display_name: "Remap",
        ports: vec![
            PortDef::input("value", BrushWireType::Scalar)
                .with_description("Input value to remap"),
            PortDef::output("result", BrushWireType::Scalar)
                .with_description("Remapped output value"),
        ],
        params: &[
            ParamDef::Float { name: "in_min", min: -1.0, max: 1.0, default: 0.0 },
            ParamDef::Float { name: "in_max", min: -1.0, max: 1.0, default: 1.0 },
            ParamDef::Float { name: "out_min", min: -1.0, max: 1.0, default: 0.0 },
            ParamDef::Float { name: "out_max", min: -1.0, max: 1.0, default: 1.0 },
        ],
        is_gpu: false,
    }
}

pub struct RemapEvaluator;

impl BrushNodeEvaluator for RemapEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let value = ctx.input_f32("value");
        let in_min = ctx.param_f32(0);
        let in_max = ctx.param_f32(1);
        let out_min = ctx.param_f32(2);
        let out_max = ctx.param_f32(3);

        let in_range = in_max - in_min;
        let t = if in_range.abs() < 1e-7 {
            0.0
        } else {
            (value - in_min) / in_range
        };
        let result = out_min + t * (out_max - out_min);
        vec![("result".into(), ScalarValue::Scalar(result))]
    }
}
