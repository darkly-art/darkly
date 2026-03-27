//! Curve node — applies a power curve (gamma) to a scalar input.
//!
//! Maps 0-1 → 0-1 via `output = input^gamma`.  Gamma < 1 produces a
//! concave curve (more sensitive at low values), gamma > 1 produces
//! convex (more sensitive at high values).  Full piecewise-linear
//! curve editing comes later.

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "curve",
        category: "math",
        display_name: "Curve",
        ports: vec![
            PortDef::input("input", BrushWireType::Scalar)
                .with_description("Input value (0\u{2013}1) to apply the power curve to"),
            PortDef::output("output", BrushWireType::Scalar)
                .with_description("Curved output (input raised to the gamma power)"),
        ],
        params: &[
            ParamDef::Float { name: "gamma", min: 0.1, max: 10.0, default: 1.0 },
        ],
        is_gpu: false,
    }
}

pub struct CurveEvaluator;

impl BrushNodeEvaluator for CurveEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let input = ctx.input_f32("input").clamp(0.0, 1.0);
        let gamma = ctx.param_f32(0).max(0.01); // prevent division by zero
        let output = input.powf(gamma);
        vec![("output".into(), ScalarValue::Scalar(output))]
    }
}
