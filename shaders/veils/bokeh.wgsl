// Bokeh blur post-processing veil.
// Golden-angle spiral disk blur with exponential brightness accumulation —
// bright pixels form characteristic circular bokeh highlights.
// Based on Shadertoy bokeh techniques by Dave Hoskins et al.
//   https://www.shadertoy.com/playlist/fXlGDN

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
    radius: f32,
    threshold: f32,
    resolution_x: f32,
    resolution_y: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

// Golden angle ≈ 137.508° ≈ 2.39996 radians.
// cos(2.39996) ≈ -0.7374, sin(2.39996) ≈ 0.6755
const GA_COS: f32 = -0.7374;
const GA_SIN: f32 = 0.6755;

@fragment fn fs_bokeh(in: VertexOutput) -> @location(0) vec4f {
    let resolution = vec2f(params.resolution_x, params.resolution_y);
    let aspect = resolution.x / resolution.y;
    let px = params.radius / resolution.y;

    // Fixed initial offset direction (pointing right).
    var p = vec2f(px, 0.0);

    let inv_t = 1.0 / max(params.threshold, 0.001);

    var acc = vec4f(0.0);
    var i = 1.0;

    // ~128 iterations: i starts at 1 and grows as i += 1/i (≈ sqrt(2n)),
    // reaching 16 after ~128 steps. Combined with the golden-angle rotation
    // this fills a disk with a sunflower/Fibonacci distribution.
    for (var n = 0; n < 128; n++) {
        if (i >= 16.0) { break; }

        // Rotate sample point by the golden angle.
        p = vec2f(
            p.x * GA_COS - p.y * GA_SIN,
            p.x * GA_SIN + p.y * GA_COS,
        );

        let offset = p * i;
        let sample_uv = in.uv + vec2f(offset.x / aspect, offset.y);
        let s = textureSampleLevel(t_input, t_sampler, sample_uv, 0.0);

        // Exponential accumulation: bright samples dominate,
        // producing the characteristic bokeh highlight shapes.
        acc += exp(s * inv_t);

        i += 1.0 / i;
    }

    // Invert the exponential and normalize via the alpha channel.
    // Alpha input is 1.0, so each sample contributes exp(1/threshold) to acc.a —
    // dividing rgb by alpha gives the smooth-maximum average, matching the
    // original Shadertoy's `O = log(O) - 5.; O /= O.a;` formulation.
    let result = log(acc) - 5.0;
    return vec4f(result.rgb / result.a, 1.0);
}
