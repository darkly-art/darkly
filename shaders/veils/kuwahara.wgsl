// Based on https://www.shadertoy.com/view/mlffWf by p4vv37

// Generalized Kuwahara filter — painterly/oil-painting post-processing veil.
// Based on work by Acerola, ported from Shadertoy GLSL to WGSL.
//   https://www.youtube.com/watch?v=LDhN-JK3U9g
//   https://github.com/GarrettGunnell/Post-Processing/tree/main/Assets/Kuwahara%20Filter

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

// Number of sectors for the generalized Kuwahara weighting.
// 8 polynomial weights are computed per sample; the final blend
// uses the first N sectors. N=4 gives a good quality/speed tradeoff.
const N: i32 = 4;

@fragment fn fs_kuwahara(in: VertexOutput) -> @location(0) vec4f {
    let uv = in.uv;
    let kernel_radius = params.kernel_size;
    let texel = 1.0 / params.resolution;

    let zeta = 2.0 / f32(kernel_radius);
    // eta = 0 for the generalized polynomial weights (zeroCross term omitted).
    let eta = 0.0;

    // Per-sector accumulators: mean (rgb + weight sum in w) and squared mean.
    var m: array<vec4f, 8>;
    var s: array<vec3f, 8>;
    for (var k = 0; k < 8; k++) {
        m[k] = vec4f(0.0);
        s[k] = vec3f(0.0);
    }

    for (var y = -kernel_radius; y <= kernel_radius; y++) {
        for (var x = -kernel_radius; x <= kernel_radius; x++) {
            let v_orig = vec2f(f32(x), f32(y)) / f32(kernel_radius);
            let c = clamp(
                textureSampleLevel(t_input, t_sampler, uv + vec2f(f32(x), f32(y)) * texel, 0.0).rgb,
                vec3f(0.0),
                vec3f(1.0),
            );

            var sum = 0.0;
            var w: array<f32, 8>;

            // Polynomial weights for the 4 axis-aligned sectors.
            let vxx = zeta - eta * v_orig.x * v_orig.x;
            let vyy = zeta - eta * v_orig.y * v_orig.y;

            var z = max(0.0, v_orig.y + vxx);
            w[0] = z * z;
            sum += w[0];

            z = max(0.0, -v_orig.x + vyy);
            w[2] = z * z;
            sum += w[2];

            z = max(0.0, -v_orig.y + vxx);
            w[4] = z * z;
            sum += w[4];

            z = max(0.0, v_orig.x + vyy);
            w[6] = z * z;
            sum += w[6];

            // Rotate 45° for the 4 diagonal sectors.
            let r2 = sqrt(2.0) / 2.0;
            let v_rot = r2 * vec2f(v_orig.x - v_orig.y, v_orig.x + v_orig.y);
            let vxx2 = zeta - eta * v_rot.x * v_rot.x;
            let vyy2 = zeta - eta * v_rot.y * v_rot.y;

            z = max(0.0, v_rot.y + vxx2);
            w[1] = z * z;
            sum += w[1];

            z = max(0.0, -v_rot.x + vyy2);
            w[3] = z * z;
            sum += w[3];

            z = max(0.0, -v_rot.y + vxx2);
            w[5] = z * z;
            sum += w[5];

            z = max(0.0, v_rot.x + vyy2);
            w[7] = z * z;
            sum += w[7];

            // Gaussian envelope weighted by polynomial sector membership.
            let g = exp(-3.125 * dot(v_orig, v_orig)) / sum;

            for (var k = 0; k < 8; k++) {
                let wk = w[k] * g;
                m[k] += vec4f(c * wk, wk);
                s[k] += c * c * wk;
            }
        }
    }

    // Blend sector means weighted by inverse variance.
    var out = vec4f(0.0);
    for (var k = 0; k < N; k++) {
        m[k] = vec4f(m[k].rgb / m[k].w, m[k].w);
        s[k] = abs(s[k] / m[k].w - m[k].rgb * m[k].rgb);

        let sigma2 = s[k].r + s[k].g + s[k].b;
        let w = 1.0 / (1.0 + pow(params.hardness * 1000.0 * sigma2, 0.5 * params.sharpness));

        out += vec4f(m[k].rgb * w, w);
    }

    return clamp(out / out.w, vec4f(0.0), vec4f(1.0));
}
