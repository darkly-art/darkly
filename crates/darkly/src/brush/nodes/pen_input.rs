//! Pen Input sensor node — source of all tablet data.
//!
//! Outputs 14 sensor values.  This node is special-cased in the runner:
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
            PortDef::output("pressure", BrushWireType::Scalar)
                .with_description("Pen pressure (0 = no pressure, 1 = full pressure)"),
            PortDef::output("x_tilt", BrushWireType::Scalar)
                .with_description("Horizontal tilt of the pen barrel (-1 = left, 1 = right)"),
            PortDef::output("y_tilt", BrushWireType::Scalar)
                .with_description("Vertical tilt of the pen barrel (-1 = toward user, 1 = away)"),
            PortDef::output("tilt_magnitude", BrushWireType::Scalar)
                .with_description("How far the pen is tilted from vertical (0 = upright, 1 = flat)"),
            PortDef::output("tilt_direction", BrushWireType::Scalar)
                .with_description("Compass direction of pen tilt (0\u{2013}1 wrapping, 0 = right)"),
            PortDef::output("rotation", BrushWireType::Scalar)
                .with_description("Barrel rotation of the pen around its own axis (0\u{2013}1)"),
            PortDef::output("tangential_pressure", BrushWireType::Scalar)
                .with_description("Pressure on the pen's side wheel/slider (Wacom Airbrush)"),
            PortDef::output("speed", BrushWireType::Scalar)
                .with_description("Stroke speed in pixels per second, normalized"),
            PortDef::output("distance", BrushWireType::Scalar)
                .with_description("Cumulative distance traveled along the stroke (pixels)"),
            PortDef::output("drawing_angle", BrushWireType::Scalar)
                .with_description("Direction of motion along the stroke (0\u{2013}1, 0 = right)"),
            PortDef::output("time", BrushWireType::Scalar)
                .with_description("Elapsed time since the stroke began (seconds)"),
            PortDef::output("position", BrushWireType::Vec2)
                .with_description("Current cursor position in canvas coordinates (x, y)"),
            PortDef::output("index", BrushWireType::Int)
                .with_description("Dab index within the current stroke (0, 1, 2, ...)"),
            PortDef::output("fade", BrushWireType::Scalar)
                .with_description("Stroke fade-out (0 at start, 1 at stroke end)"),
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
