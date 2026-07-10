//! Noise node — procedural 2D cell noise sampled per fragment.
//!
//! Outputs a grayscale RGBA `color` whose channels all hold the same
//! noise value, so the same downstream chain a texture-driven brush
//! uses (`noise → split_color → luminance → …`) keeps working when the
//! texture is swapped for procedural noise. Charcoal uses this for
//! paper grain — no `.jpg` asset, no `@group(3)` binding, just an
//! integer-cell PCG hash inlined into the compiled shader.
//!
//! Each integer cell is hashed independently; there is no
//! interpolation between cells. Adjacent fragments that don't share
//! a cell read uncorrelated values, which is the "every pixel is a
//! random value" shape: at `scale = 1` every fragment is its own
//! random sample; larger scales group pixels into hard-edged cells
//! that share one hashed value.
//!
//! Coordinate frame matches [`super::image`]: `target_pos` (canvas
//! pixels in stroke mode, preview-mask texels in preview mode) divided
//! by `scale` gives the cell size in pixels.
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
            description: "Procedural noise sampled where the brush touches the canvas — for grain, jitter, and texture.",
            ports: vec![
                PortDef::output("color", BrushWireType::Vec4).with_description(
                    "Grayscale RGBA value noise at the fragment's canvas-pixel position",
                ),
            ],
            params: &[
                // Canvas pixels per noise cell. `scale = 1` gives
                // a unique hash per fragment (true per-pixel noise);
                // larger values group pixels into hard-edged cells
                // sharing one hash. Sub-pixel scales sweep multiple
                // cells per fragment, which still reads as per-pixel
                // noise but with a different sampling phase. Capping
                // at 16 keeps the cell from approaching the dab's
                // own radius (where it would flatten to a single
                // hashed value).
                ParamDef::Float {
                    name: "scale",
                    min: 0.1,
                    max: 16.0,
                    default: 1.0,
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
            preview_fallback_icon: None,
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
        // Tiny epsilon floor guards the divide; the param min itself
        // (0.1) keeps the user-facing range inside the useful zone.
        let scale = cctx
            .params
            .first()
            .and_then(param_as_f32)
            .unwrap_or(1.0)
            .max(1e-3);
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

// ── CPU mirror of the WGSL noise functions ──────────────────────────────
//
// Byte-equivalent (up to floating-point reassociation) to the helpers
// in `shaders/brush/_noise.wgsl`. Used to render the brush-builder's
// in-node preview thumbnail without round-tripping through the GPU —
// a 96 × 96 tile is ~9k pixels, one hash per pixel, comfortably
// sub-millisecond.
//
// Keep these in lockstep with the WGSL versions — if the shader
// algorithm changes (e.g. switch to Gaussian distribution, add
// blurring, etc.) these CPU mirrors must change the same way or the
// preview lies to the user about what they'll see on canvas.

fn cpu_pcg(n: u32) -> u32 {
    let mut h = n.wrapping_mul(747796405).wrapping_add(2891336453);
    let shift = (h >> 28).wrapping_add(4);
    h = ((h >> shift) ^ h).wrapping_mul(277803737);
    (h >> 22) ^ h
}

fn cpu_hash2(cx: i32, cy: i32, seed: u32) -> f32 {
    let cxu = cx as u32;
    let cyu = cy as u32;
    let h = cpu_pcg(cxu.wrapping_add(cpu_pcg(cyu.wrapping_add(cpu_pcg(seed)))));
    h as f32 / u32::MAX as f32
}

fn cpu_noise_value(px: f32, py: f32, seed: u32) -> f32 {
    cpu_hash2(px.floor() as i32, py.floor() as i32, seed)
}

/// Render a square noise preview tile and PNG-encode it. Called by
/// the engine's `brush_node_preview` for noise-type nodes. Synchronous —
/// the work is small enough that an async readback is more ceremony
/// than the operation deserves.
pub fn render_preview_png(params: &[ParamValue], size: u32) -> Vec<u8> {
    let scale = params
        .first()
        .and_then(param_as_f32)
        .unwrap_or(1.0)
        .max(1e-3);
    let seed = params.get(1).and_then(param_as_u32).unwrap_or(1);

    let mut img = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let n = cpu_noise_value(x as f32 / scale, y as f32 / scale, seed).clamp(0.0, 1.0);
            let v = (n * 255.0) as u8;
            let i = ((y * size + x) * 4) as usize;
            img[i] = v;
            img[i + 1] = v;
            img[i + 2] = v;
            img[i + 3] = 255;
        }
    }
    crate::engine::rendering::encode_rgba_as_png(&img, size, size)
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

    #[test]
    fn cpu_noise_is_deterministic_per_seed() {
        // Same coord + same seed → same value across calls.
        let a = cpu_noise_value(3.7, 12.1, 42);
        let b = cpu_noise_value(3.7, 12.1, 42);
        assert_eq!(a, b);
        // Different seed → different value almost surely.
        let c = cpu_noise_value(3.7, 12.1, 43);
        assert!((a - c).abs() > 1e-6, "seed must perturb the hash");
    }

    #[test]
    fn cpu_noise_stays_in_unit_range() {
        // The shader's range is [0, 1); CPU mirror must agree so the
        // preview pixels don't clamp or wrap unexpectedly.
        for y in 0..50 {
            for x in 0..50 {
                let n = cpu_noise_value(x as f32 * 0.31, y as f32 * 0.47, 7);
                assert!((0.0..=1.0).contains(&n), "x={x} y={y} n={n} out of [0, 1]");
            }
        }
    }

    #[test]
    fn render_preview_png_returns_png_bytes() {
        let params = vec![ParamValue::Float(2.0), ParamValue::Int(1)];
        let png = render_preview_png(&params, 32);
        assert!(!png.is_empty(), "preview PNG must be non-empty");
        assert_eq!(
            &png[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "preview must be a valid PNG"
        );
    }
}
