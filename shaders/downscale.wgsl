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

@fragment fn fs_downscale(in: VertexOutput) -> @location(0) vec4f {
    // Output pixel size in input-UV space. We take its absolute value
    // because the vertex shader flips V, so dpdy may be negative.
    let footprint = vec2f(abs(dpdx(in.uv.x)), abs(dpdy(in.uv.y)));

    // 4 bilinear taps at the centers of the 4 quadrants of the output
    // pixel's input footprint — i.e. ±¼ of the footprint from center
    // along each axis. At exactly 2× downscale this lands each tap at a
    // half-texel position so bilinear filtering averages 2 input texels
    // and the 4 taps tile the 2×2 input area exactly. At lighter
    // downscales (e.g. 1.4× for scale=0.7) the taps fall closer
    // together and we still get a clean weighted box.
    let tap = footprint * 0.25;

    let s00 = textureSample(t_input, t_sampler, in.uv + vec2f(-tap.x, -tap.y));
    let s10 = textureSample(t_input, t_sampler, in.uv + vec2f( tap.x, -tap.y));
    let s01 = textureSample(t_input, t_sampler, in.uv + vec2f(-tap.x,  tap.y));
    let s11 = textureSample(t_input, t_sampler, in.uv + vec2f( tap.x,  tap.y));
    return 0.25 * (s00 + s10 + s01 + s11);
}
