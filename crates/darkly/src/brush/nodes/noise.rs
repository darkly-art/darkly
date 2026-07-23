//! Noise node — procedural, domain-warped, per-octave-rotated fBm sampled
//! per fragment.
//!
//! Outputs a chromatic RGBA `color`: each channel is an independent fBm
//! field driven by `seed + {0,1,2}`, so R, G, and B carry uncorrelated
//! noise — genuine color grain / cloud. For a monochrome field (paper
//! grain, scatter masks) desaturate downstream via `noise → split_color
//! → luminance`, which averages the three channels back into one scalar.
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
//! The math (`fbm_value_noise`, `fbm_seed_xform`, `fbm_rot`, hash, fade)
//! lives in the shared `shaders/lib/fbm2d.wgsl`, concatenated into every
//! assembled brush shader; the WGSL compiler dead-strips it when no node
//! calls through. See that file for credits.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{frame_sample_coord_expr, CompileWgslCtx, NodeWgsl, SampleFrame};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef, PortDir, UnitType};

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
                // sampling — the same input-port path `shape.rotation_input`
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
                PortDef::output("color", BrushWireType::Vec4).with_description(
                    "Chromatic RGBA fBm noise at the fragment's sample position — each channel an independent field",
                ),
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
        // `scale`/`octaves`/`warp`/`roughness` are wirable Scalar inputs, so
        // each resolves to a WGSL expression — a `{:.6}` literal when unwired,
        // an upstream expr when wired. The runtime clamps that used to be
        // applied to compile-time literals now live in the emitted WGSL so
        // they hold for a wired value too.
        let scale_expr = cctx.input("scale").as_f32();
        let octaves_expr = format!(
            "clamp(i32(round(({}))), 1, 8)",
            cctx.input("octaves").as_f32()
        );
        let warp_expr = format!("max(({}), 0.0)", cctx.input("warp").as_f32());
        let gain_expr = format!("clamp(({}), 0.0, 1.0)", cctx.input("roughness").as_f32());
        // `seed` is baked as a `{seed}u` literal (per-channel/per-octave
        // offsets are folded in at compile time), so it's read as a
        // compile-time integer, not an expression.
        let seed = cctx.input("seed").enum_index().max(0) as u32;

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

        let [r_seed, g_seed, b_seed] = CHANNEL_SEED_OFFSETS.map(|o| seed.wrapping_add(o));
        let var = cctx.ident("noise_c");
        wgsl.body = format!(
            "{frame_pre}\
             \x20   let {var}_p = {coord};\n\
             \x20   let {var} = vec4<f32>(\n\
             \x20       fbm_rot({var}_p, {r_seed}u, {octaves_expr}, {gain_expr}, {warp_expr}),\n\
             \x20       fbm_rot({var}_p, {g_seed}u, {octaves_expr}, {gain_expr}, {warp_expr}),\n\
             \x20       fbm_rot({var}_p, {b_seed}u, {octaves_expr}, {gain_expr}, {warp_expr}),\n\
             \x20       1.0);\n"
        );
        wgsl.outputs.insert("color".into(), var);
        Ok(wgsl)
    }
}

// ── CPU mirror of the WGSL fBm functions ────────────────────────────────
//
// Byte-equivalent (up to floating-point reassociation) to the helpers in
// `shaders/lib/fbm2d.wgsl`: `fbm_value_noise`, `fbm_seed_xform`, `fbm_rot`.
// Used to render the brush-builder's in-node preview thumbnail without
// round-tripping through the GPU.
//
// Keep these in lockstep with the WGSL versions — including the exact
// rotation/offset constants (`i*0.9`, `i*13.7`, `i*7.1`, the `64.0` seed
// offset scale, the warp offsets `(11.5,3.7)`/`(5.2,1.3)`). If the shader
// algorithm or any constant changes, these mirrors must change the same
// way or the preview lies to the user about what they'll see on canvas.

/// Per-channel seed offsets for the R/G/B channels (alpha is opaque).
/// PCG decorrelates adjacent seeds, so three consecutive seeds drive three
/// independent fields. Shared by the WGSL emitter and the CPU mirror below —
/// the two must apply the same offsets or the preview diverges from canvas.
const CHANNEL_SEED_OFFSETS: [u32; 3] = [0, 1, 2];

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

fn cpu_fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Interpolated 2D value noise — bilinear blend of the four corner hashes
/// through the quintic fade. Mirrors `fbm_value_noise`.
fn cpu_noise_value(px: f32, py: f32, seed: u32) -> f32 {
    let ix = px.floor() as i32;
    let iy = py.floor() as i32;
    let wx = cpu_fade(px - ix as f32);
    let wy = cpu_fade(py - iy as f32);
    let a = cpu_hash2(ix, iy, seed);
    let b = cpu_hash2(ix + 1, iy, seed);
    let c = cpu_hash2(ix, iy + 1, seed);
    let d = cpu_hash2(ix + 1, iy + 1, seed);
    let ab = a + (b - a) * wx;
    let cd = c + (d - c) * wx;
    ab + (cd - ab) * wy
}

/// Seed → (base angle, offset x, offset y). Mirrors `fbm_seed_xform`
/// (the WGSL uses a `6.28318530718` literal for TAU; the ~1e-7 difference
/// from `std::f32::consts::TAU` is immaterial for a decorrelation angle).
fn cpu_seed_xform(seed: u32) -> (f32, f32, f32) {
    let a = cpu_pcg(seed) as f32 / u32::MAX as f32 * std::f32::consts::TAU;
    let ox = cpu_pcg(seed.wrapping_add(101)) as f32 / u32::MAX as f32 * 64.0;
    let oy = cpu_pcg(seed.wrapping_add(202)) as f32 / u32::MAX as f32 * 64.0;
    (a, ox, oy)
}

/// Per-octave rotated, domain-warped fBm scalar. Mirrors `fbm_rot`.
fn cpu_fbm(px: f32, py: f32, seed: u32, octaves: i32, gain: f32, warp: f32) -> f32 {
    let (base_a, ox, oy) = cpu_seed_xform(seed);
    let mut cx = px;
    let mut cy = py;
    if warp > 0.0 {
        let wx = cpu_noise_value(cx + 11.5, cy + 3.7, seed);
        let wy = cpu_noise_value(cx + 5.2, cy + 1.3, seed);
        cx += warp * (wx - 0.5);
        cy += warp * (wy - 0.5);
    }
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut norm = 0.0;
    let n = octaves.max(1);
    for i in 0..n {
        let ang = base_a + i as f32 * 0.9;
        let (sa, ca) = ang.sin_cos();
        let sx = cx * freq;
        let sy = cy * freq;
        let rx = sx * ca - sy * sa + ox + i as f32 * 13.7;
        let ry = sx * sa + sy * ca + oy + i as f32 * 7.1;
        sum += amp * cpu_noise_value(rx, ry, seed.wrapping_add((i as u32).wrapping_mul(1013)));
        norm += amp;
        freq *= 2.0;
        amp *= gain;
    }
    sum / norm
}

/// Chromatic mirror of the shader's `vec4` output — three independent fBm
/// fields under the offset seeds, alpha opaque. Mirrors the WGSL emitter's
/// per-channel [`CHANNEL_SEED_OFFSETS`] fanout.
fn cpu_noise_color(px: f32, py: f32, seed: u32, octaves: i32, gain: f32, warp: f32) -> [f32; 4] {
    let [r, g, b] =
        CHANNEL_SEED_OFFSETS.map(|o| cpu_fbm(px, py, seed.wrapping_add(o), octaves, gain, warp));
    [r, g, b, 1.0]
}

/// Render a square noise preview tile and PNG-encode it. Called by
/// the engine's `brush_node_preview` for noise-type nodes. Synchronous —
/// the work is small enough that an async readback is more ceremony
/// than the operation deserves. Reads its knobs from the node's input
/// port values (the unified input model).
pub fn render_preview_png(ports: &[PortDef<BrushWireType>], size: u32) -> Vec<u8> {
    let input = |name: &str, fallback: f32| -> f32 {
        ports
            .iter()
            .find(|p| p.name == name && p.dir == PortDir::Input)
            .map(|p| p.value.as_f32())
            .unwrap_or(fallback)
    };
    let scale = input("scale", 32.0).max(1e-3);
    let seed = input("seed", 1.0).max(0.0) as u32;
    let octaves = (input("octaves", 4.0) as i32).clamp(1, 8);
    let warp = input("warp", 0.6).max(0.0);
    let gain = input("roughness", 0.5).clamp(0.0, 1.0);

    let mut img = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let c = cpu_noise_color(
                x as f32 / scale,
                y as f32 / scale,
                seed,
                octaves,
                gain,
                warp,
            );
            let i = ((y * size + x) * 4) as usize;
            img[i] = (c[0].clamp(0.0, 1.0) * 255.0) as u8;
            img[i + 1] = (c[1].clamp(0.0, 1.0) * 255.0) as u8;
            img[i + 2] = (c[2].clamp(0.0, 1.0) * 255.0) as u8;
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
        // rotation, variation, scale, seed, octaves, warp, roughness, space,
        // scale_with_brush inputs plus the color output — all unified.
        assert_eq!(reg.node.ports.len(), 10);
        assert!(reg.node.ports.iter().any(|p| p.name == "color"));
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

    #[test]
    fn cpu_noise_is_interpolated_not_cell() {
        // Regression: the field must be smooth, not blocky. Two coordinates
        // in the SAME integer cell but at different fractional positions must
        // produce DIFFERENT values — cell noise (`hash(floor(p))`) returns the
        // identical hash for both (this test fails against it); interpolated
        // value noise blends the corner hashes, so it must not.
        let a = cpu_noise_value(3.10, 3.10, 7);
        let b = cpu_noise_value(3.15, 3.10, 7);
        assert!(
            (a - b).abs() > 1e-6,
            "sub-cell samples must differ (interpolated, not cell noise): a={a} b={b}"
        );
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
    fn cpu_noise_color_channels_are_independent() {
        // Real color: the three channels are uncorrelated fBm fields, not a
        // broadcast of one scalar. Alpha stays opaque.
        let c = cpu_noise_color(3.7, 12.1, 42, 4, 0.5, 0.6);
        assert_eq!(c[3], 1.0, "alpha must be opaque");
        assert!((c[0] - c[1]).abs() > 1e-6, "r and g must differ: {c:?}");
        assert!((c[1] - c[2]).abs() > 1e-6, "g and b must differ: {c:?}");
        assert!((c[0] - c[2]).abs() > 1e-6, "r and b must differ: {c:?}");
    }

    #[test]
    fn cpu_noise_color_red_matches_scalar_fbm() {
        // The R channel is the base seed (offset 0), so it equals the plain
        // scalar fBm — the mono path (split_color → luminance) stays
        // consistent with what R carries.
        let seed = 9;
        assert_eq!(
            cpu_noise_color(2.2, 5.5, seed, 4, 0.5, 0.6)[0],
            cpu_fbm(2.2, 5.5, seed, 4, 0.5, 0.6),
        );
    }

    #[test]
    fn cpu_fbm_stays_in_unit_range() {
        // The renormalized fBm stays in ~[0, 1]; the CPU mirror must agree so
        // preview pixels don't clamp or wrap. Small tolerance for float
        // reassociation in the octave sum.
        for y in 0..50 {
            for x in 0..50 {
                let n = cpu_fbm(x as f32 * 0.31, y as f32 * 0.47, 7, 5, 0.5, 0.6);
                assert!(
                    (-0.001..=1.001).contains(&n),
                    "x={x} y={y} n={n} out of [0, 1]"
                );
            }
        }
    }

    #[test]
    fn render_preview_png_returns_png_bytes() {
        // Read the knobs straight off the registration's input ports.
        let png = render_preview_png(&register().node.ports, 32);
        assert!(!png.is_empty(), "preview PNG must be non-empty");
        assert_eq!(
            &png[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "preview must be a valid PNG"
        );
    }
}
