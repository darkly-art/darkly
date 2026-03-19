//! Pen Input sensor node — source of all tablet data.
//!
//! Outputs 16 sensor values.  This node is special-cased in the runner:
//! `seed_sensors()` writes directly to its output slots (no virtual
//! dispatch).  The evaluator is a no-op.

use crate::brush::wire::BrushWireType;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "pen_input",
        category: "sensor",
        display_name: "Pen Input",
        ports: vec![
            PortDef::output("pressure", BrushWireType::Scalar),
            PortDef::output("x_tilt", BrushWireType::Scalar),
            PortDef::output("y_tilt", BrushWireType::Scalar),
            PortDef::output("tilt_magnitude", BrushWireType::Scalar),
            PortDef::output("tilt_direction", BrushWireType::Scalar),
            PortDef::output("rotation", BrushWireType::Scalar),
            PortDef::output("tangential_pressure", BrushWireType::Scalar),
            PortDef::output("speed", BrushWireType::Scalar),
            PortDef::output("distance", BrushWireType::Scalar),
            PortDef::output("drawing_angle", BrushWireType::Scalar),
            PortDef::output("time", BrushWireType::Scalar),
            PortDef::output("position", BrushWireType::Vec2),
            PortDef::output("index", BrushWireType::Int),
            PortDef::output("fuzzy_dab", BrushWireType::Scalar),
            PortDef::output("fuzzy_stroke", BrushWireType::Scalar),
            PortDef::output("fade", BrushWireType::Scalar),
        ],
        params: &[],
        is_gpu: false,
    }
}

/// No-op evaluator — `seed_sensors()` handles this node directly.
pub struct PenInputEvaluator;

impl BrushNodeEvaluator for PenInputEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        // Slots are written by seed_sensors(), not by the evaluator.
        vec![]
    }
}
