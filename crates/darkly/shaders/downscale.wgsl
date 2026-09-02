// Multi-tap soft downscale used by the veil chain when a veil renders at
// reduced resolution. Replaces the single-tap bilinear blit, which acts as
// a fixed 2×2 box filter regardless of the downscale ratio and aliases
// hard for filters (like Painting) whose output is hypersensitive to
// small input differences.
//
// Each output pixel takes 4 bilinear taps positioned at the corners of
// its footprint in the input texture. The footprint size is derived from
// screen-space derivatives — `dpdx(uv)` is the change in input UV per
// output pixel in X, i.e. exactly one output pixel's width in input-UV
// space — so the shader self-adapts to any source/destination ratio
// without needing a uniform.

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

// Alpha-weighted mean of four texels. Canvas accumulators hold *straight*
// (non-premultiplied) alpha — composite.wgsl divides its result by `out_a` —
// so a fully transparent texel carries RGB 0 and an unweighted average across
// an alpha edge drags colour toward black: opaque red averaged with empty
// gives half-alpha *dark* red rather than half-alpha red. Weighting colour by
// coverage and carrying alpha as its own plain mean is the fix, and it is a
// no-op wherever alpha is already 1 everywhere. Same idiom as the disc blur in
// lib/aberration.wgsl, which weights its taps for the same reason.
fn weighted_mean(s0: vec4f, s1: vec4f, s2: vec4f, s3: vec4f) -> vec4f {
    let acc_rgb = s0.rgb * s0.a + s1.rgb * s1.a + s2.rgb * s2.a + s3.rgb * s3.a;
    let acc_a = s0.a + s1.a + s2.a + s3.a;
    return vec4f(acc_rgb / max(acc_a, 1.0e-4), acc_a * 0.25);
}

@fragment fn fs_downscale(in: VertexOutput) -> @location(0) vec4f {
    // Output pixel size in input-UV space. We take its absolute value
    // because the vertex shader flips V, so dpdy may be negative.
    let footprint = vec2f(abs(dpdx(in.uv.x)), abs(dpdy(in.uv.y)));

    // 4 taps at the centers of the 4 quadrants of the output pixel's input
    // footprint — i.e. ±¼ of the footprint from center along each axis. At
    // exactly 2× downscale that tiles the 2×2 input area; at lighter ratios
    // (1.41× at the default scale) the taps fall closer together and still
    // give a clean box.
    //
    // `textureLoad`, not `textureSample`: a bilinear tap would have the
    // hardware average up to four straight-alpha texels *inside* each tap,
    // before this shader can weight anything, which is the same colour error
    // the weighting exists to remove.
    let dims = vec2f(textureDimensions(t_input));
    let tap = footprint * 0.25;
    let hi = vec2i(dims) - vec2i(1, 1);

    let p00 = clamp(vec2i((in.uv + vec2f(-tap.x, -tap.y)) * dims), vec2i(0), hi);
    let p10 = clamp(vec2i((in.uv + vec2f( tap.x, -tap.y)) * dims), vec2i(0), hi);
    let p01 = clamp(vec2i((in.uv + vec2f(-tap.x,  tap.y)) * dims), vec2i(0), hi);
    let p11 = clamp(vec2i((in.uv + vec2f( tap.x,  tap.y)) * dims), vec2i(0), hi);

    return weighted_mean(
        textureLoad(t_input, p00, 0),
        textureLoad(t_input, p10, 0),
        textureLoad(t_input, p01, 0),
        textureLoad(t_input, p11, 0),
    );
}
