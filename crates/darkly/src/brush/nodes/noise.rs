//! Noise node — procedural 2D value noise sampled per fragment.
//!
//! Outputs a grayscale RGBA `color` whose channels all hold the same
//! noise value, so the same downstream chain a texture-driven brush
//! uses (`noise → split_color → luminance → …`) keeps working when the
//! texture is swapped for procedural noise. Charcoal uses this for
//! paper grain — no `.jpg` asset, no `@group(3)` binding, just hash +
//! bilinear blend inlined into the compiled shader.
//!
//! Coordinate frame matches [`super::image`]: `target_pos` (canvas
//! pixels in stroke mode, preview-mask texels in preview mode) divided
//! by `scale` gives the noise lattice spacing. Value noise is
//! infinitely tileable on its own — no `fract` wrap needed; bigger
//! `scale` = coarser features.
//!
//! Helpers (`node_noise_value`, hash, fade) live in
//! `shaders/brush/_noise.wgsl` and are always linked into the assembled
//! brush shader; the WGSL compiler dead-strips them when no node calls
//! through. See that file for credits.
//!
//! Restoration note: the previous Charcoal layout sampled a
//! `paper-charcoal.jpg` texture through the [`super::image`] node and
//! depended on the engine's `TextureRegistry` to bind it at
//! `@group(3)`. Procedural noise removes that runtime dependency and
//! frees the `@group(3)` slot for terminal use.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::gpu::params::{ParamDef, ParamValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "noise";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "texture",
            display_name: "Noise",
            ports: vec![
                PortDef::output("color", BrushWireType::Vec4).with_description(
                    "Grayscale RGBA value noise at the fragment's canvas-pixel position",
                ),
            ],
            params: &[
                ParamDef::Float {
                    name: "scale",
                    min: 1.0,
                    max: 4096.0,
                    default: 8.0,
                },
                ParamDef::Int {
                    name: "seed",
                    min: 0,
                    max: 65535,
                    default: 1,
                },
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
        },
        || Box::new(NoiseEvaluator),
    )
}

pub struct NoiseEvaluator;

impl BrushNodeEvaluator for NoiseEvaluator {
    /// CPU evaluation returns a neutral grey — `noise` is only
    /// meaningful per-fragment. Same shape as [`super::image`]'s CPU
    /// stub for the same reason.
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![("color".into(), ScalarValue::Vec4([0.5, 0.5, 0.5, 1.0]))]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("color") {
            return Ok(wgsl);
        }
        let scale = cctx
            .params
            .first()
            .and_then(param_as_f32)
            .unwrap_or(8.0)
            .max(1.0);
        let seed = cctx.params.get(1).and_then(param_as_u32).unwrap_or(1);

        let var = cctx.ident("noise_c");
        wgsl.body = format!(
            "    let {var}_n = node_noise_value(target_pos / {scale:.6}, {seed}u);\n\
             \x20   let {var} = vec4<f32>({var}_n, {var}_n, {var}_n, 1.0);\n"
        );
        wgsl.outputs.insert("color".into(), var);
        Ok(wgsl)
    }
}

fn param_as_f32(p: &ParamValue) -> Option<f32> {
    match p {
        ParamValue::Float(v) => Some(*v),
        ParamValue::Int(v) => Some(*v as f32),
        _ => None,
    }
}

fn param_as_u32(p: &ParamValue) -> Option<u32> {
    match p {
        ParamValue::Int(v) => Some((*v).max(0) as u32),
        ParamValue::Float(v) => Some(v.max(0.0) as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_shape() {
        let reg = register();
        assert_eq!(reg.node.type_id, "noise");
        assert_eq!(reg.node.category, "texture");
        assert_eq!(reg.node.ports.len(), 1);
        assert_eq!(reg.node.ports[0].name, "color");
        assert_eq!(reg.node.params.len(), 2);
    }
}
