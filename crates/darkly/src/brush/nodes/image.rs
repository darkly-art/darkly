//! Image node — sample a named texture in the compiled fragment shader.
//!
//! Looks up `texture_name` in
//! [`crate::gpu::texture_registry::TextureRegistry`] at brush-load time.
//! The texture is bound at `@group(3)` of the compiled stroke shader
//! (see [`crate::brush::wgsl`]); the node emits a single `textureSample`
//! call at a coordinate chosen by the `space` param (shared with
//! [`super::noise`] through
//! [`crate::brush::wgsl::frame_sample_coord_expr`]), wrapped in `fract`.
//!
//! Sampling frame: because an `image` node is a brush *tip*, it defaults
//! to **Dab** space — the picture rides the stamp, rotating and
//! translating with each dab (`local_uv` rotated by the `rotation` input).
//! **Canvas** space is also available for users who tile a picture as a
//! fixed canvas texture: it anchors the pattern to the canvas so
//! overlapping strokes share phase. `fract(...)` wraps cleanly in either
//! frame because the registry's shared sampler uses repeat addressing.
//!
//! Restoration note: an `image` node existed before the WGSL
//! migration. That version returned a runtime texture handle on a
//! `BrushWireType::Texture` wire and downstream nodes received it
//! as a per-dab value. The current node is shaped for the
//! WGSL-compiled pipeline: it doesn't move texture data through
//! wires — it inlines a `textureSample` call into the compiled
//! shader and the binding lives in the per-brush pipeline. The
//! output is `color` (Vec4); scalar consumers chain through
//! [`super::split_color`].

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{frame_sample_coord_expr, CompileWgslCtx, NodeWgsl, SampleFrame};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::gpu::params::{ParamDef, ParamValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub const TYPE_ID: &str = "image";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "texture",
            display_name: "Image",
            description: "Uses a picture from the brush bundle as the brush tip shape.",
            ports: vec![
                // Per-dab orientation and decorrelation for Dab-space
                // sampling — the same input-port path `shape.rotation_input`
                // uses. Hidden in Canvas mode.
                PortDef::input("rotation", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Rotation")
                    .with_unit(UnitType::Degrees)
                    .with_visible_when("space", [1])
                    .with_description(
                        "Per-dab orientation (radians) for Dab space. Wire pen direction here so the tip follows the stroke.",
                    ),
                PortDef::input("variation", BrushWireType::Scalar)
                    .with_range(0.0, 1024.0, 0.0)
                    .with_natural_range(0.0, 1024.0)
                    .with_label("Variation")
                    .with_unit(UnitType::Raw)
                    .with_visible_when("space", [1])
                    .with_description(
                        "Per-dab decorrelation offset for Dab space. Wire random (Per-Dab) so overlapping dabs sample independent regions.",
                    ),
                PortDef::output("color", BrushWireType::Vec4)
                    .with_description("RGBA value sampled from the named texture at the fragment's sample position"),
            ],
            params: &[
                ParamDef::String {
                    name: "texture_name",
                    default: "",
                },
                ParamDef::Float {
                    name: "scale",
                    min: 1.0,
                    max: 4096.0,
                    default: 512.0,
                },
                // A tip picture rides the stamp, so Dab is the default; Canvas
                // is offered for tiling a picture as a fixed canvas texture.
                ParamDef::Enum {
                    name: "space",
                    options: &["Canvas", "Dab"],
                    default: 1,
                },
                // Dab-space only: `true` scales the picture with the brush,
                // `false` keeps its texel density constant in canvas pixels.
                ParamDef::Bool {
                    name: "scale_with_brush",
                    default: true,
                },
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_fallback_icon: None,
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
        vec![("color".into(), ScalarValue::Vec4([0.5, 0.5, 0.5, 1.0]))]
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
                ParamValue::Float(v) => Some(*v),
                ParamValue::Int(v) => Some(*v as f32),
                _ => None,
            })
            .unwrap_or(512.0)
            .max(1.0);
        let space = SampleFrame::from_index(
            cctx.params
                .get(2)
                .and_then(|p| match p {
                    ParamValue::Int(v) => Some((*v).max(0) as u32),
                    _ => None,
                })
                .unwrap_or(1),
        );
        let scale_with_brush = cctx
            .params
            .get(3)
            .and_then(|p| match p {
                ParamValue::Bool(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(true);
        let rotation = cctx.input("rotation").as_f32();
        let variation = cctx.input("variation").as_f32();
        let (frame_pre, coord) = frame_sample_coord_expr(
            space,
            scale,
            scale_with_brush,
            &rotation,
            &variation,
            &cctx.ident("img"),
        );

        let slot = cctx.request_texture(texture_name);
        let var = cctx.ident("img_c");
        // The frame helper's `coord` is canvas-pixel space in Canvas mode
        // and the stamp's oriented frame in Dab mode; both wrap cleanly
        // through `fract`. The shared sampler `graph_smp` is bound at
        // `@group(3) @binding(0)`; the texture lives at `@binding(1 + slot)`.
        let sample = crate::brush::wgsl::sample_graph_texture(slot, &format!("fract({coord})"));
        wgsl.body = format!("{frame_pre}    let {var} = {sample};\n");
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
        // rotation + variation inputs plus the color output.
        assert_eq!(reg.node.ports.len(), 3);
        assert!(reg.node.ports.iter().any(|p| p.name == "color"));
        assert!(reg.node.ports.iter().any(|p| p.name == "rotation"));
        assert!(reg.node.ports.iter().any(|p| p.name == "variation"));
        // texture_name, scale, space, scale_with_brush.
        assert_eq!(reg.node.params.len(), 4);
    }
}
