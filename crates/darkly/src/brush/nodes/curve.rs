//! Curve node — applies an adjustable spline transfer function to a scalar input.
//!
//! Maps 0-1 → 0-1 via a monotone cubic Hermite spline defined by user-placed
//! control points.  The spline is precomputed into a 256-entry LUT at graph
//! compile time (see `BrushGraphRunner`), so per-dab evaluation is a single
//! O(1) table lookup.
//!
//! Prior art: Krita's `KisCubicCurve` and GIMP's `GimpCurve` both use
//! precomputed LUTs for brush dynamics curves.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

const DEFAULT_CURVE: &[[f32; 2]] = &[[0.0, 0.0], [1.0, 1.0]];

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(NodeRegistration {
        type_id: "curve",
        category: "modulate",
        display_name: "Curve",
        ports: vec![
            PortDef::input("input", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description("Input value (0\u{2013}1) to remap through the curve"),
            PortDef::output("output", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description("Remapped output from the spline transfer function"),
        ],
        params: &[ParamDef::Curve {
            name: "curve",
            default: DEFAULT_CURVE,
        }],
        is_gpu: false,
    })
}

pub struct CurveEvaluator;

impl BrushNodeEvaluator for CurveEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let input = ctx.input_f32("input").clamp(0.0, 1.0);
        let output = ctx.curve_lookup(input);
        vec![("output".into(), ScalarValue::Scalar(output))]
    }
}
