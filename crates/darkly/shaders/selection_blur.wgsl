// Separable Gaussian blur of an R8 selection mask (one directional pass).
//
// Feather (user radius) and antialias (a fixed ~1px radius) both run a
// horizontal pass then a vertical pass over this shader. σ and the kernel
// radius are supplied by the engine, matching `crate::mask::gaussian_kernel`
// (σ = radius / 2, kernel extent ±ceil(radius)).

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

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct Params {
    dir: vec2f, // texel step along the blur axis (1/size on one axis, 0 on the other)
    radius: i32,
    sigma: f32,
}
@group(0) @binding(2) var<uniform> params: Params;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let two_sigma_sq = 2.0 * params.sigma * params.sigma;
    var acc = 0.0;
    var wsum = 0.0;
    for (var i = -params.radius; i <= params.radius; i = i + 1) {
        let fi = f32(i);
        let w = exp(-(fi * fi) / two_sigma_sq);
        acc = acc + w * textureSample(src_tex, samp, in.uv + params.dir * fi).r;
        wsum = wsum + w;
    }
    return vec4f(acc / wsum, 0.0, 0.0, 1.0);
}
