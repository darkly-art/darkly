//! Paint Color node — outputs the current foreground color.
//!
//! Like pen_input, this node is special-cased: the runner seeds its
//! output slot directly with the stroke's foreground color.

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl_compile::{CompileWgslCtx, NodeWgsl, UniformField, WgslType};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(NodeRegistration {
        type_id: "paint_color",
        category: "color",
        display_name: "Paint Color",
        ports: vec![PortDef::output("color", BrushWireType::Color)
            .with_description("Current foreground painting color (RGBA)")],
        params: &[],
        is_gpu: false,
    })
}

/// No-op evaluator — `seed_sensors()` handles this node directly.
pub struct PaintColorEvaluator;

impl BrushNodeEvaluator for PaintColorEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// The stroke's foreground color is constant for every dab, so it
    /// goes into the uniform buffer (one copy per stroke), not the
    /// per-dab record. Only emitted if a downstream node consumes the
    /// color output.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("color") {
            return Ok(wgsl);
        }
        let field_name = cctx.uniform_field_name("color");
        let key = field_name.clone();
        wgsl.uniform_fields.push(UniformField {
            name: field_name.clone(),
            ty: WgslType::Vec4,
            pack: Arc::new(move |outputs, bytes| {
                let v = outputs
                    .get(&key)
                    .map(|s| s.as_color())
                    .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                bytes.extend_from_slice(bytemuck::bytes_of(&v));
            }),
        });
        wgsl.outputs
            .insert("color".into(), format!("u.{}", field_name));
        Ok(wgsl)
    }
}
