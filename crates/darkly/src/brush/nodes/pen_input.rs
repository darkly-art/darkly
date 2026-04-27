//! Pen Input sensor node — source of all tablet data.
//!
//! Outputs 14 sensor values.  This node is special-cased in the runner:
//! `seed_sensors()` writes directly to its output slots (no virtual
//! dispatch).  The evaluator is a no-op.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

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
            PortDef::output("tilt_magnitude", BrushWireType::Scalar).with_description(
                "How far the pen is tilted from vertical (0 = upright, 1 = flat)",
            ),
            PortDef::output("tilt_direction", BrushWireType::Scalar)
                .with_description("Compass direction of pen tilt in radians (0 = right, π/2 = down)"),
            PortDef::output("rotation", BrushWireType::Scalar)
                .with_description("Barrel rotation of the pen around its own axis (0\u{2013}1)"),
            PortDef::output("tangential_pressure", BrushWireType::Scalar)
                .with_description("Pressure on the pen's side wheel/slider (Wacom Airbrush)"),
            PortDef::output("speed", BrushWireType::Scalar)
                .with_description("Stroke speed in pixels per second, normalized"),
            PortDef::output("distance", BrushWireType::Scalar)
                .with_description("Cumulative distance traveled along the stroke (pixels)"),
            PortDef::output("drawing_angle", BrushWireType::Scalar)
                .with_description("Direction of motion along the stroke in radians (0 = right, π/2 = down). Wire to `stamp.rotation` for brushes that face the stroke."),
            PortDef::output("time", BrushWireType::Scalar)
                .with_description("Elapsed time since the stroke began (seconds)"),
            PortDef::output("position", BrushWireType::Vec2)
                .with_description("Current cursor position in canvas coordinates (x, y)"),
            PortDef::output("motion", BrushWireType::Vec2).with_description(
                "Per-dab motion vector in canvas pixels (delta from previous sample)",
            ),
            PortDef::output("index", BrushWireType::Int)
                .with_description("Dab index within the current stroke (0, 1, 2, ...)"),
            PortDef::output("fade", BrushWireType::Scalar)
                .with_description("Stroke fade-out (0 at start, 1 at stroke end)"),
            // Stabilization strength — input port read at stroke start,
            // not per-dab.  Exposed via the eye toggle like any other port.
            PortDef::input("stabilize", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-wave-square")
                .with_label("Stabilize")
                .with_description(
                    "Stroke stabilization strength (0 = off, 100% = maximum smoothing)",
                ),
            // Dab spacing — read at stroke start as a fraction of the dab
            // diameter. Like `stabilize`, this is brush-level config that
            // currently lives here because the engine reads pen_input port
            // defaults out-of-band; both move together when the brush
            // settings bar gets redesigned.
            PortDef::input("spacing", BrushWireType::Scalar)
                .with_range(0.04, 1.0, 0.10)
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-grip-lines-vertical")
                .with_label("Spacing")
                .with_description(
                    "Distance between dabs as a fraction of dab diameter. \
                     10% is the paint default; warp/smudge brushes typically want 4\u{2013}5%. \
                     Floor of 4% — anything lower swamps the stabilizer.",
                ),
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
