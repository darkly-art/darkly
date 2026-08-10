//! Stamp dab node.
//!
//! Inlines `color × mask` into the brush's WGSL via [`compile_wgsl`].
//! The upstream `tip` input is a scalar coverage expression (typically
//! from `circle.mask`'s compile output); the emitted `dab` output is
//! premultiplied RGBA that downstream paint terminals consume.
//!
//! Flow lives on the `paint` terminal — that is the single, authoritative
//! per-stroke deposit knob. Dab dimensions, rotation, and mirroring are
//! likewise owned by the terminal (which sizes its quad from
//! `bbox_target_px`).

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "stamp";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![],
        evaluator: || Box::new(StampEvaluator),
        lifecycle: crate::brush::node::Lifecycle::None,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "shape",
            display_name: "Stamp Tip",
            description: "Combines a tip shape with a color to form a colored brush mark. Feed this into a Paint output to deposit it on the canvas.",
            ports: vec![
                PortDef::input("tip", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Per-fragment tip coverage (0..1)"),
                PortDef::input("color", BrushWireType::Vec4).with_description("Brush color (RGBA)"),
                PortDef::output("dab", BrushWireType::Vec4)
                    .preview_image()
                    .with_description("The stamped brush mark (premultiplied RGBA)"),
            ],
            is_gpu: true,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
        },
    }
}

pub struct StampEvaluator;

impl BrushNodeEvaluator for StampEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// Inline `color × mask` into the brush's WGSL. `tip` carries the
    /// upstream scalar coverage expression; the emitted `dab` output is
    /// premultiplied RGBA. Flow is applied later by the `paint` terminal.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("dab") {
            return Ok(wgsl);
        }
        let mask = cctx.input("tip").as_f32();
        let color = cctx.input("color").as_vec4();

        let fn_name = cctx.ident("stamp");
        let decls = format!(
            "fn {fn_name}(mask: f32, color: vec4<f32>) -> vec4<f32> {{\n\
             \x20   let a = color.a * mask;\n\
             \x20   return vec4<f32>(color.rgb * a, a);\n\
             }}\n"
        );
        wgsl.decls = decls;
        wgsl.outputs
            .insert("dab".into(), format!("{}({}, {})", fn_name, mask, color));
        Ok(wgsl)
    }
}
