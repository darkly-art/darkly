//! Image node — sample a named texture in the compiled fragment shader.
//!
//! Looks up `texture_name` in
//! [`crate::gpu::texture_registry::TextureRegistry`] at brush-load time.
//! The texture is bound at `@group(4)` of the compiled stroke shader
//! (see [`crate::brush::wgsl`]); the node emits a single
//! `textureSample` call against canvas-pixel space, divided by
//! `scale` to control how the texture tiles across the canvas.
//!
//! Why canvas-space and not stamp-local: charcoal-style brushes want
//! the paper grain anchored to the canvas, so multiple overlapping
//! strokes share the same texture phase (light strokes register on
//! the high points, heavy strokes fill the valleys — a single
//! coherent sheet of paper). The fragment shader already exposes the
//! per-fragment canvas-pixel position as `target_pos: vec2<f32>`,
//! and `fract(target_pos / scale)` wraps cleanly because the
//! registry's shared sampler is configured for repeat addressing.
//!
//! Restoration note: an `image` node existed before the WGSL
//! migration. That version returned a runtime `TextureHandle`
//! (`BrushWireType::Texture`) and downstream nodes received the
//! handle as a per-dab value. The current node is shaped for the
//! WGSL-compiled pipeline: it doesn't move texture data through
//! wires — it inlines a `textureSample` call into the compiled
//! shader and the binding lives in the per-brush pipeline. The
//! output is `color` (Vec4); scalar consumers chain through
//! [`super::split_color`].

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "image";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "texture",
            display_name: "Image",
            ports: vec![PortDef::output("color", BrushWireType::Color)
                .with_description("RGBA value sampled from the named texture at the fragment's canvas-pixel position")],
            params: &[
                ParamDef::String {
                    name: "texture_name",
                    default: "paper-charcoal",
                },
                ParamDef::Float {
                    name: "scale",
                    min: 1.0,
                    max: 4096.0,
                    default: 512.0,
                },
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
        },
        || Box::new(ImageEvaluator),
    )
}

pub struct ImageEvaluator;

impl BrushNodeEvaluator for ImageEvaluator {
    /// CPU evaluation returns a neutral grey — `image` is only
    /// meaningful per-fragment, and the per-dab CPU dispatch path is
    /// dead for compiled-WGSL brushes. The constant exists so brushes
    /// that mix CPU and compiled execution don't `NaN` through the
    /// `color` port.
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![("color".into(), ScalarValue::Color([0.5, 0.5, 0.5, 1.0]))]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("color") {
            // Nothing downstream consumes the sample — skip the
            // `textureSample` and don't reserve a binding either.
            return Ok(wgsl);
        }
        let texture_name = cctx
            .params
            .first()
            .and_then(|p| match p {
                crate::gpu::params::ParamValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        if texture_name.is_empty() {
            return Err("image node: `texture_name` is empty".into());
        }
        let scale = cctx
            .params
            .get(1)
            .and_then(|p| match p {
                crate::gpu::params::ParamValue::Float(v) => Some(*v),
                crate::gpu::params::ParamValue::Int(v) => Some(*v as f32),
                _ => None,
            })
            .unwrap_or(512.0)
            .max(1.0);

        let slot = cctx.request_texture(texture_name);
        let var = cctx.ident("img_c");
        // `target_pos` is canvas-pixel space in stroke mode and
        // preview-mask texels in preview mode — both wrap cleanly
        // through `fract`. The shared sampler `graph_smp` is bound at
        // `@group(4) @binding(0)`; the texture lives at
        // `@binding(1 + slot)`.
        wgsl.body = format!(
            "    let {var} = textureSample(graph_tex_{slot}, graph_smp, \
             fract(target_pos / {scale:.6}));\n"
        );
        wgsl.outputs.insert("color".into(), var);
        Ok(wgsl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_shape() {
        let reg = register();
        assert_eq!(reg.node.type_id, "image");
        assert_eq!(reg.node.category, "texture");
        // One output (color) and two params (texture_name, scale).
        assert_eq!(reg.node.ports.len(), 1);
        assert_eq!(reg.node.ports[0].name, "color");
        assert_eq!(reg.node.params.len(), 2);
    }
}
