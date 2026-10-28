/// Compute shader: stretch a single-dab render so its strongest texel reaches
/// full contrast against the background it was drawn over.
///
/// Two entry points share one bind group and run as consecutive dispatches:
/// `reduce` folds each texel's deviation from `zero` into `peak`, then `apply`
/// scales every texel's deviation by `full / peak` (identity when the dab is
/// empty or already at full contrast) and writes the result to `dst`. Alpha is
/// carried through unchanged.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> peak: atomic<u32>;

struct Params {
    /// The background colour; `.w` is unused.
    zero: vec4f,
    /// The deviation from `zero` the strongest texel is stretched to.
    full: f32,
    width: u32,
    height: u32,
}
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var dst: texture_storage_2d<rgba8unorm, write>;

/// The source texel as an 8-bit target would have stored it: NaN reads as 0
/// and everything else clamps to [0, 1], so a float source cannot push the
/// peak out of range. NaN is found by its bit pattern, which a compiler that
/// assumes finite floats cannot fold away.
fn texel(p: vec2u) -> vec4f {
    let v = textureLoad(src, p, 0);
    let nan = (bitcast<vec4u>(v) & vec4u(0x7fffffffu)) > vec4u(0x7f800000u);
    return saturate(select(v, vec4f(0.0), nan));
}

fn deviation(c: vec4f) -> f32 {
    let d = abs(c.rgb - params.zero.rgb);
    return max(d.r, max(d.g, d.b));
}

@compute @workgroup_size(16, 16)
fn reduce(@builtin(global_invocation_id) gid: vec3u) {
    if gid.x >= params.width || gid.y >= params.height { return; }
    let d = deviation(texel(gid.xy));
    // Non-negative floats order as their bits, so an integer max is a float max.
    if d > 0.0 {
        atomicMax(&peak, bitcast<u32>(d));
    }
}

@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) gid: vec3u) {
    if gid.x >= params.width || gid.y >= params.height { return; }
    let c = texel(gid.xy);
    let p = bitcast<f32>(atomicLoad(&peak));
    let k = select(1.0, params.full / p, p > 0.0 && p < params.full);
    let rgb = clamp(params.zero.rgb + (c.rgb - params.zero.rgb) * k, vec3f(0.0), vec3f(1.0));
    textureStore(dst, gid.xy, vec4f(rgb, c.a));
}
