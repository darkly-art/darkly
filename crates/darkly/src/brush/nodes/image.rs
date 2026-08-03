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
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{frame_sample_coord_expr, CompileWgslCtx, NodeWgsl, SampleFrame};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
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
                // sampling — the same input-port path `circle.rotation_input`
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
                // Named texture, resolved at compile time — not wirable.
                PortDef::input("texture_name", BrushWireType::String)
                    .with_value(InputValue::String(String::new()))
                    .with_label("Texture")
                    .with_description("Name of the picture in the brush bundle to sample."),
                // Feature size in canvas pixels. A per-dab-computable scalar,
                // so it's wirable (drive it from pressure, a curve, etc.).
                PortDef::input("scale", BrushWireType::Scalar)
                    .with_range(1.0, 4096.0, 512.0)
                    .with_natural_range(1.0, 4096.0)
                    .with_label("Scale")
                    .with_unit(UnitType::Pixels)
                    .with_description("Base feature size in canvas pixels."),
                // A tip picture rides the stamp, so Dab is the default; Canvas
                // is offered for tiling a picture as a fixed canvas texture.
                PortDef::input("space", BrushWireType::Enum)
                    .with_enum_options(["Canvas", "Dab"])
                    .with_value(InputValue::Int(1))
                    .with_label("Space")
                    .with_description("Sample in canvas space (pinned) or the dab's oriented frame."),
                // Dab-space only: `true` scales the picture with the brush,
                // `false` keeps its texel density constant in canvas pixels.
                PortDef::input("scale_with_brush", BrushWireType::Bool)
                    .with_value(InputValue::Bool(true))
                    .with_label("Scale With Brush")
                    .with_description("Dab space only: scale the picture with the brush size."),
                PortDef::output("color", BrushWireType::Vec4)
                    .preview_image()
                    .with_description("RGBA value sampled from the named texture at the fragment's sample position"),
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
        let texture_name = cctx.input("texture_name").string();
        if texture_name.is_empty() {
            return Err("image node: `texture_name` is empty".into());
        }
        // `scale` is a wirable Scalar input, so it resolves to a WGSL
        // expression (a literal when unwired, an upstream expr when wired).
        let scale_expr = cctx.input("scale").as_f32();
        let space = SampleFrame::from_index(cctx.input("space").enum_index().max(0) as u32);
        let scale_with_brush = cctx.input("scale_with_brush").boolean();
        let rotation = cctx.input("rotation").as_f32();
        let variation = cctx.input("variation").as_f32();
        // The sampled uv is `fract`-wrapped against a repeat sampler, so the
        // texture is effectively periodic with period 1.0 — the per-dab
        // decorrelation offset must span exactly one period.
        let (frame_pre, coord) = frame_sample_coord_expr(
            space,
            &scale_expr,
            scale_with_brush,
            &rotation,
            &variation,
            1.0,
            &cctx.ident("img"),
        );

        let slot = cctx.request_texture(&texture_name);
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
        // rotation, variation, texture_name, scale, space, scale_with_brush
        // inputs plus the color output — all unified as ports now.
        assert_eq!(reg.node.ports.len(), 7);
        assert!(reg.node.ports.iter().any(|p| p.name == "color"));
        assert!(reg.node.ports.iter().any(|p| p.name == "rotation"));
        assert!(reg.node.ports.iter().any(|p| p.name == "variation"));
        assert!(reg.node.ports.iter().any(|p| p.name == "texture_name"));
        assert!(reg.node.ports.iter().any(|p| p.name == "scale"));
        assert!(reg.node.ports.iter().any(|p| p.name == "space"));
        assert!(reg.node.ports.iter().any(|p| p.name == "scale_with_brush"));
    }
}
