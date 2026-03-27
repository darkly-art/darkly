//! User-exposed property node — a labeled scalar source that appears in the
//! brush properties panel.
//!
//! Functionally identical to `constant` — outputs a Scalar from a parameter.
//! Semantically distinct: the system surfaces all `user_input` nodes as
//! labeled sliders in the user-facing properties panel, giving brush creators
//! a way to expose named controls without requiring end users to open the
//! node graph.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "user_input",
        category: "input",
        display_name: "User Input",
        ports: vec![
            PortDef::output("value", BrushWireType::Scalar)
                .with_description("The user-controlled slider value (0\u{2013}1)"),
        ],
        params: &[
            ParamDef::String { name: "label", default: "" },
            ParamDef::Float { name: "value", min: 0.0, max: 1.0, default: 0.5 },
        ],
        is_gpu: false,
    }
}

pub struct UserInputEvaluator;

impl BrushNodeEvaluator for UserInputEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        // param 0 = label (String, unused at eval time)
        // param 1 = value (Float)
        let value = ctx.param_f32(1);
        vec![("value".into(), ScalarValue::Scalar(value))]
    }
}
