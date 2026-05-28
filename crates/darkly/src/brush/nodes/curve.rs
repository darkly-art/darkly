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
use crate::brush::wgsl_compile::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

const DEFAULT_CURVE: &[[f32; 2]] = &[[0.0, 0.0], [1.0, 1.0]];

pub const TYPE_ID: &str = "curve";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(NodeRegistration {
        type_id: TYPE_ID,
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
        is_terminal: false,
        supports_erase: true,
    })
}

pub struct CurveEvaluator;

impl BrushNodeEvaluator for CurveEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let input = ctx.input_f32("input").clamp(0.0, 1.0);
        let output = ctx.curve_lookup(input);
        vec![("output".into(), ScalarValue::Scalar(output))]
    }

    /// Embeds the precomputed 256-entry LUT as a WGSL `const array<f32, 256>`
    /// and emits a 2-tap linear lookup function. Per-fragment evaluation
    /// matches the CPU's `curve_lookup` to within float precision.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("output") {
            return Ok(wgsl);
        }
        let lut = cctx
            .lut
            .ok_or_else(|| "curve node missing LUT".to_string())?;

        let fn_name = cctx.ident("curve");
        let mut decls = format!(
            "const {}_LUT: array<f32, 256> = array<f32, 256>(\n",
            fn_name
        );
        for (i, v) in lut.table().iter().enumerate() {
            decls.push_str(&format!("    {:.7},", v));
            if i % 8 == 7 {
                decls.push('\n');
            }
        }
        if !decls.ends_with('\n') {
            decls.push('\n');
        }
        decls.push_str(");\n");
        decls.push_str(&format!(
            "fn {fn_name}(t: f32) -> f32 {{\n\
             \x20   let idx = clamp(t, 0.0, 1.0) * 255.0;\n\
             \x20   let lo = u32(floor(idx));\n\
             \x20   let hi = min(lo + 1u, 255u);\n\
             \x20   let f = idx - floor(idx);\n\
             \x20   return mix({fn_name}_LUT[lo], {fn_name}_LUT[hi], f);\n\
             }}\n"
        ));

        wgsl.decls = decls;
        let input_expr = cctx.input("input").as_f32();
        wgsl.outputs
            .insert("output".into(), format!("{}({})", fn_name, input_expr));
        Ok(wgsl)
    }
}
