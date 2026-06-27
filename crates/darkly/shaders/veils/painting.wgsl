// Based on https://www.shadertoy.com/view/mlffWf by p4vv37

// Generalized Kuwahara filter — painterly/oil-painting post-processing veil.
// Based on work by Acerola, ported from Shadertoy GLSL to WGSL.
//   https://www.youtube.com/watch?v=LDhN-JK3U9g
//   https://github.com/GarrettGunnell/Post-Processing/tree/main/Assets/Kuwahara%20Filter
//
// We specialize on the 4 axis-aligned (cardinal) sectors. The reference
// implementation computes 8 sectors per sample (cardinal + 45°-rotated
// diagonals) but blends only the first 4, so the diagonal weights and
// accumulators were pure waste — this version drops them.

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

struct Params {
    kernel_size: i32,
    sharpness: f32,
    hardness: f32,
    _pad: f32,
    resolution: vec2f,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

@fragment fn fs_painting(in: VertexOutput) -> @location(0) vec4f {
    let uv = in.uv;
    let kernel_radius = params.kernel_size;
    let texel = 1.0 / params.resolution;
    let inv_r = 1.0 / f32(kernel_radius);

    let zeta = 2.0 * inv_r;
    // eta = 0 for the generalized polynomial weights (zeroCross term omitted),
    // so the polynomial inputs reduce to plain `zeta` — hoisted out of the loop.

    // Per-sector accumulators for the 4 cardinal sectors:
    // mean (rgb + weight sum in w) and squared mean (rgb).
    var m0 = vec4f(0.0);
    var m1 = vec4f(0.0);
    var m2 = vec4f(0.0);
    var m3 = vec4f(0.0);
    var s0 = vec3f(0.0);
    var s1 = vec3f(0.0);
    var s2 = vec3f(0.0);
    var s3 = vec3f(0.0);

    for (var y = -kernel_radius; y <= kernel_radius; y++) {
        for (var x = -kernel_radius; x <= kernel_radius; x++) {
            let v = vec2f(f32(x), f32(y)) * inv_r;
            // Rgba8Unorm already guarantees [0,1] — no clamp needed.
            let c = textureSampleLevel(t_input, t_sampler, uv + vec2f(f32(x), f32(y)) * texel, 0.0).rgb;

            // Polynomial weights for the 4 axis-aligned sectors.
            let zy_pos = max(0.0, v.y + zeta);
            let zx_neg = max(0.0,-v.x + zeta);
            let zy_neg = max(0.0,-v.y + zeta);
            let zx_pos = max(0.0, v.x + zeta);

            let w0 = zy_pos * zy_pos;
            let w1 = zx_neg * zx_neg;
            let w2 = zy_neg * zy_neg;
            let w3 = zx_pos * zx_pos;
            // `sum` normalizes each sample's polynomial weights so its total
            // contribution across sectors equals the Gaussian envelope. The
            // 8-sector reference summed all 8 weights; with 4 cardinal
            // sectors we sum only those four (which still gives a smooth
            // Gaussian spatial fall-off).
            let inv_sum = 1.0 / (w0 + w1 + w2 + w3);

            let g = exp(-3.125 * dot(v, v)) * inv_sum;
            let cc = c * c;

            let wg0 = w0 * g;
            let wg1 = w1 * g;
            let wg2 = w2 * g;
            let wg3 = w3 * g;

            m0 += vec4f(c * wg0, wg0);
            m1 += vec4f(c * wg1, wg1);
            m2 += vec4f(c * wg2, wg2);
            m3 += vec4f(c * wg3, wg3);
            s0 += cc * wg0;
            s1 += cc * wg1;
            s2 += cc * wg2;
            s3 += cc * wg3;
        }
    }

    // Blend sector means weighted by inverse variance.
    let h_pow = 0.5 * params.sharpness;
    let h_scale = params.hardness * 1000.0;

    var out = vec4f(0.0);

    let mean0 = m0.rgb / m0.w;
    let var0 = abs(s0 / m0.w - mean0 * mean0);
    let bw0 = 1.0 / (1.0 + pow(h_scale * (var0.r + var0.g + var0.b), h_pow));
    out += vec4f(mean0 * bw0, bw0);

    let mean1 = m1.rgb / m1.w;
    let var1 = abs(s1 / m1.w - mean1 * mean1);
    let bw1 = 1.0 / (1.0 + pow(h_scale * (var1.r + var1.g + var1.b), h_pow));
    out += vec4f(mean1 * bw1, bw1);

    let mean2 = m2.rgb / m2.w;
    let var2 = abs(s2 / m2.w - mean2 * mean2);
    let bw2 = 1.0 / (1.0 + pow(h_scale * (var2.r + var2.g + var2.b), h_pow));
    out += vec4f(mean2 * bw2, bw2);

    let mean3 = m3.rgb / m3.w;
    let var3 = abs(s3 / m3.w - mean3 * mean3);
    let bw3 = 1.0 / (1.0 + pow(h_scale * (var3.r + var3.g + var3.b), h_pow));
    out += vec4f(mean3 * bw3, bw3);

    return vec4f(out.rgb / out.w, 1.0);
}
