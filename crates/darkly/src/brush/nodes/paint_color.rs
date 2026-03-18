//! Paint Color node — outputs the current foreground color.
//!
//! Like pen_input, this node is special-cased: the runner seeds its
//! output slot directly with the stroke's foreground color.

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "paint_color",
        category: "color",
        display_name: "Paint Color",
        ports: vec![
            PortDef::output("color", BrushWireType::Color),
        ],
        params: &[],
        is_gpu: false,
    }
}

/// No-op evaluator — `seed_sensors()` handles this node directly.
pub struct PaintColorEvaluator;

impl BrushNodeEvaluator for PaintColorEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }
}
