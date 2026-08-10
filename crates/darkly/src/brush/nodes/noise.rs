//! Noise node — procedural, domain-warped, per-octave-rotated fBm sampled
//! per fragment.
//!
//! Outputs a chromatic RGBA `color`: each channel is an independent fBm
//! field driven by `seed + {0,1,2}`, so R, G, and B carry uncorrelated
//! noise — genuine color grain / cloud. For a monochrome field (paper
//! grain, scatter masks) take the scalar `value` output instead — a single
//! fBm field at the base seed, one third the per-fragment cost of computing
//! three channels and averaging them back down.
//!
//! The field is **interpolated value noise** (bilinear blend of corner
//! hashes through a quintic fade — smooth and resolution-independent, not
//! blocky cell noise), summed over `octaves` at doubling frequency, with a
//! single-octave domain warp and a per-octave rotation that break the
//! axis-aligned value-noise lattice. It never visibly repeats. `scale`
//! sets the base feature size in canvas pixels; `warp` the domain
//! distortion; `roughness` the per-octave amplitude falloff.
//!
//! Coordinate frame is selectable via the `space` param, shared with
//! [`super::image`] through [`crate::brush::wgsl::frame_sample_coord_expr`]:
//! **Canvas** (default) samples `target_pos / scale` — canvas pixels in
//! stroke mode, preview-mask texels in preview — so the grain is pinned to
//! the canvas. **Dab** samples the stamp's oriented unit frame (`local_uv`
//! rotated by the `rotation` input, offset per dab by `variation`), so the
//! grain rides the stamp instead of swimming under it. `scale_with_brush`
//! chooses whether Dab-frame grain scales with the brush or stays
//! pixel-locked.
//!
//! The math (`fbm_value_noise`, `fbm_seed_xform`, `fbm_tile`, hash, fade)
//! lives in the shared `shaders/lib/fbm2d.wgsl`, concatenated into every
//! assembled brush shader; the WGSL compiler dead-strips it when no node
//! calls through. See that file for credits.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::texture_source::{BakeChannels, BakeKind, BakeSpec, ResolvedSource};
use crate::brush::wgsl::{
    frame_sample_coord_expr, sample_graph_texture, CompileWgslCtx, NodeWgsl, SampleFrame,
};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub const TYPE_ID: &str = "noise";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "texture",
            display_name: "Noise",
            description: "Procedural noise sampled where the brush touches the canvas — for grain, jitter, and texture.",
            ports: vec![
                // Per-dab orientation and decorrelation for Dab-space
                // sampling — the same input-port path `circle.rotation_input`
                // uses. Hidden in Canvas mode, where the grain is pinned to
                // the canvas and neither applies.
                PortDef::input("rotation", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Rotation")
                    .with_unit(UnitType::Degrees)
                    .with_visible_when("space", [1])
                    .with_description(
                        "Per-dab orientation (radians) for Dab space. Wire pen direction here so the grain follows the stroke.",
                    ),
                PortDef::input("variation", BrushWireType::Scalar)
                    .with_range(0.0, 1024.0, 0.0)
                    .with_natural_range(0.0, 1024.0)
                    .with_label("Variation")
                    .with_unit(UnitType::Raw)
                    .with_visible_when("space", [1])
                    .with_description(
                        "Per-dab decorrelation offset for Dab space. Wire random (Per-Dab) so overlapping dabs show independent grain.",
                    ),
                // Base feature size in canvas pixels: `target_pos / scale`
                // sets the lowest octave's cell size. A per-dab-computable
                // scalar — wirable (drive it from pressure, a curve, …).
                PortDef::input("scale", BrushWireType::Scalar)
                    .with_range(1.0, 512.0, 32.0)
                    .with_natural_range(1.0, 512.0)
                    .with_label("Scale")
                    .with_unit(UnitType::Pixels)
                    .with_description("Base feature size in canvas pixels."),
                // RNG seed. A compile-time integer baked into `{seed}u`
                // literals (the per-channel/per-octave offsets are computed at
                // compile time), so wiring it has no per-dab effect.
                PortDef::input("seed", BrushWireType::Int)
                    .with_range(0.0, 65535.0, 1.0)
                    .with_value(InputValue::Int(1))
                    .with_step(1.0)
                    .with_label("Seed")
                    .with_unit(UnitType::Raw)
                    .with_description("RNG seed for the noise field."),
                // Number of fBm octaves — each adds detail at 2× frequency,
                // `roughness×` amplitude. Wirable.
                PortDef::input("octaves", BrushWireType::Scalar)
                    .with_range(1.0, 8.0, 4.0)
                    .with_natural_range(1.0, 8.0)
                    .with_step(1.0)
                    .with_label("Octaves")
                    .with_unit(UnitType::Raw)
                    .with_description("Number of stacked fBm frequencies."),
                // Domain-warp strength. 0 = pure fBm; higher smears the field
                // into a marbled, organic distortion. Wirable.
                PortDef::input("warp", BrushWireType::Scalar)
                    .with_range(0.0, 2.5, 0.6)
                    .with_natural_range(0.0, 2.5)
                    .with_label("Warp")
                    .with_description("Domain-warp strength."),
                // Per-octave amplitude falloff (gain). Lower = smoother;
                // higher = grainier. Wirable.
                PortDef::input("roughness", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.5)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Roughness")
                    .with_unit(UnitType::Percent)
                    .with_description("Per-octave amplitude falloff."),
                // Coordinate frame the field is sampled in. Canvas pins the
                // grain to the canvas (default); Dab locks it to the stamp.
                PortDef::input("space", BrushWireType::Enum)
                    .with_enum_options(["Canvas", "Dab"])
                    .with_value(InputValue::Int(0))
                    .with_label("Space")
                    .with_description("Pin the grain to the canvas, or lock it to each dab."),
                // Dab-space only: `true` scales the grain with the brush,
                // `false` keeps grain density constant in canvas pixels.
                PortDef::input("scale_with_brush", BrushWireType::Bool)
                    .with_value(InputValue::Bool(true))
                    .with_label("Scale With Brush")
                    .with_description("Dab space only: scale the grain with the brush size."),
                PortDef::output("color", BrushWireType::Vec4)
                    .preview_image()
                    .with_description(
                    "Chromatic RGBA fBm noise at the fragment's sample position — each channel an independent field",
                ),
                // Monochrome scalar field — a single fBm evaluation at the base
                // seed (`color.r`'s field), for grain / masks that only need one
                // channel. Emitted independently of `color`, so a value-only
                // consumer pays one `fbm_tile`, not three.
                PortDef::output("value", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description(
                        "Monochrome fBm noise at the sample position — a single scalar field for grain and masks",
                    ),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
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
        vec![
            ("color".into(), ScalarValue::Vec4([0.5, 0.5, 0.5, 1.0])),
            ("value".into(), ScalarValue::Scalar(0.5)),
        ]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let want_color = cctx.consumed_outputs.contains("color");
        let want_value = cctx.consumed_outputs.contains("value");
        if !want_color && !want_value {
            return Ok(wgsl);
        }
        // The sampling frame is shared by the baked and the live path.
        // `scale`/`space`/`scale_with_brush`/`rotation`/`variation` shape the
        // sample *coordinate*, not the field content — so they are applied
        // here at sample time and one baked tile serves Canvas and Dab, every
        // scale and every per-dab variation.
        let scale_expr = cctx.input("scale").as_f32();
        let space = SampleFrame::from_index(cctx.input("space").enum_index().max(0) as u32);
        let scale_with_brush = cctx.input("scale_with_brush").boolean();
        let rotation = cctx.input("rotation").as_f32();
        let variation = cctx.input("variation").as_f32();
        let (frame_pre, coord) = frame_sample_coord_expr(
            space,
            &scale_expr,
            scale_with_brush,
            &rotation,
            &variation,
            &cctx.ident("noise"),
        );
        // `seed` is a compile-time integer in both paths (a wired `Int`
        // degrades to `0` via `enum_index`).
        let seed = cctx.input("seed").enum_index().max(0) as u32;
        let var = cctx.ident("noise_c");

        // Bake the field into a cached tile when every field-defining input
        // is static — turning the ~80-hash fBm kernel (re-run per fragment
        // per overlapping dab) into a single `textureSample`. Any wired field
        // input falls through to the live kernel below.
        let field_static = ["octaves", "warp", "roughness"]
            .iter()
            .all(|p| !cctx.input_is_wired(p));

        if field_static {
            // Read the concrete field params and re-apply the node's exact
            // clamps in Rust, so the baked tile matches the live path at the
            // clamp boundaries (the bake shader must not re-clamp).
            let octaves = cctx
                .input("octaves")
                .as_f32_literal()
                .unwrap_or(4.0)
                .round()
                .clamp(1.0, 8.0) as i32;
            let warp = cctx.input("warp").as_f32_literal().unwrap_or(0.6).max(0.0);
            let gain = cctx
                .input("roughness")
                .as_f32_literal()
                .unwrap_or(0.5)
                .clamp(0.0, 1.0);
            let kind = BakeKind::Noise {
                seed,
                octaves,
                warp_q: BakeKind::quantize(warp),
                roughness_q: BakeKind::quantize(gain),
            };
            let resolution = BakeSpec::resolution_for_octaves(octaves);

            // The tile packs `TILE_SPAN` field units across its `[0,1)` uv, so
            // the sample coordinate is divided by `TILE_SPAN` (and wrapped):
            // feature size then matches the live path, and the field repeats
            // once per `TILE_SPAN` field units — the seam lever (§ plan).
            let mut body = format!("{frame_pre}    let {var}_p = {coord};\n");
            let uv = format!("fract(({var}_p) / {:.1})", BakeSpec::TILE_SPAN);
            if want_value {
                let slot = cctx.request_source(ResolvedSource::Baked(BakeSpec {
                    kind,
                    channels: BakeChannels::Grayscale,
                    resolution,
                }));
                let val = cctx.ident("noise_v");
                let sample = sample_graph_texture(slot, &uv);
                body.push_str(&format!("    let {val} = ({sample}).r;\n"));
                wgsl.outputs.insert("value".into(), val);
            }
            if want_color {
                let slot = cctx.request_source(ResolvedSource::Baked(BakeSpec {
                    kind,
                    channels: BakeChannels::Rgba,
                    resolution,
                }));
                let cvar = cctx.ident("noise_rgba");
                let sample = sample_graph_texture(slot, &uv);
                body.push_str(&format!("    let {cvar} = {sample};\n"));
                wgsl.outputs.insert("color".into(), cvar);
            }
            wgsl.body = body;
            return Ok(wgsl);
        }

        // Live fallback — a wired field input means the tile can't be
        // precomputed. `octaves`/`warp`/`roughness` resolve to WGSL
        // expressions; the clamps live in the emitted WGSL so they hold for
        // the wired value too.
        let octaves_expr = format!(
            "clamp(i32(round(({}))), 1, 8)",
            cctx.input("octaves").as_f32()
        );
        let warp_expr = format!("max(({}), 0.0)", cctx.input("warp").as_f32());
        let gain_expr = format!("clamp(({}), 0.0, 1.0)", cctx.input("roughness").as_f32());

        // The sampled coordinate is shared by both outputs — emit it once,
        // then let each consumed output reference `{var}_p`. The live field
        // uses the same tileable `fbm_tile` and the same period the baked path
        // does, so a wired-param brush and a static one look identical.
        let period = BakeSpec::TILE_SPAN as i32;
        let mut body = format!("{frame_pre}    let {var}_p = {coord};\n");

        if want_value {
            let val = cctx.ident("noise_v");
            body.push_str(&format!(
                "    let {val} = fbm_tile({var}_p, {seed}u, {octaves_expr}, {gain_expr}, {warp_expr}, {period});\n"
            ));
            wgsl.outputs.insert("value".into(), val);
        }
        if want_color {
            let [r_seed, g_seed, b_seed] = CHANNEL_SEED_OFFSETS.map(|o| seed.wrapping_add(o));
            body.push_str(&format!(
                "    let {var} = vec4<f32>(\n\
                 \x20       fbm_tile({var}_p, {r_seed}u, {octaves_expr}, {gain_expr}, {warp_expr}, {period}),\n\
                 \x20       fbm_tile({var}_p, {g_seed}u, {octaves_expr}, {gain_expr}, {warp_expr}, {period}),\n\
                 \x20       fbm_tile({var}_p, {b_seed}u, {octaves_expr}, {gain_expr}, {warp_expr}, {period}),\n\
                 \x20       1.0);\n"
            ));
            wgsl.outputs.insert("color".into(), var);
        }
        wgsl.body = body;
        Ok(wgsl)
    }
}

/// Per-channel seed offsets for the R/G/B channels (alpha is opaque).
/// PCG decorrelates adjacent seeds, so three consecutive seeds drive three
/// independent fields. The WGSL emitter applies these offsets to fan one
/// seed into three uncorrelated fBm channels.
const CHANNEL_SEED_OFFSETS: [u32; 3] = [0, 1, 2];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_shape() {
        let reg = register();
        assert_eq!(reg.node.type_id, "noise");
        assert_eq!(reg.node.category, "texture");
        // rotation, variation, scale, seed, octaves, warp, roughness, space,
        // scale_with_brush inputs plus the color and value outputs — all unified.
        assert_eq!(reg.node.ports.len(), 11);
        assert!(reg.node.ports.iter().any(|p| p.name == "color"));
        assert!(reg.node.ports.iter().any(|p| p.name == "value"));
        assert!(reg.node.ports.iter().any(|p| p.name == "rotation"));
        assert!(reg.node.ports.iter().any(|p| p.name == "variation"));
        for name in [
            "scale",
            "seed",
            "octaves",
            "warp",
            "roughness",
            "space",
            "scale_with_brush",
        ] {
            assert!(
                reg.node.ports.iter().any(|p| p.name == name),
                "missing {name}"
            );
        }
    }
}
