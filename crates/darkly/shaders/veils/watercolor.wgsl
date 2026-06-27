// Watercolor post-processing veil.
// Iterative directional blur in CMYK space with texture-based flow-map bias,
// based on Shadertoy watercolor simulations.
//   https://www.shadertoy.com/view/mdlXW2

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
    pass_type: i32,        // 0 = RGB→CMYK, 1 = blur iteration, 2 = CMYK→RGB
    wetness: f32,
    resolution_x: f32,
    resolution_y: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var t_noise: texture_2d<f32>;
@group(0) @binding(4) var t_noise_sampler: sampler;

// --- CMYK ↔ RGB conversion ---
// From https://gist.github.com/mattdesl/e40d3189717333293813626cbdb2c1d1
// ("Graphics Shaders")

fn cmyk_to_rgb(cmyk: vec4f) -> vec3f {
    let inv_k = 1.0 - cmyk.w;
    let r = 1.0 - min(1.0, cmyk.x * inv_k + cmyk.w);
    let g = 1.0 - min(1.0, cmyk.y * inv_k + cmyk.w);
    let b = 1.0 - min(1.0, cmyk.z * inv_k + cmyk.w);
    return clamp(vec3f(r, g, b), vec3f(0.0), vec3f(1.0));
}

fn rgb_to_cmyk(rgb: vec3f) -> vec4f {
    let k = min(1.0 - rgb.r, min(1.0 - rgb.g, 1.0 - rgb.b));
    var cmy = vec3f(0.0);
    let inv_k = 1.0 - k;
    if (inv_k > 0.0) {
        cmy = vec3f(
            (1.0 - rgb.r - k) / inv_k,
            (1.0 - rgb.g - k) / inv_k,
            (1.0 - rgb.b - k) / inv_k,
        );
    }
    return clamp(vec4f(cmy, k), vec4f(0.0), vec4f(1.0));
}

// Sample the flow-map bias from the noise texture, matching the original
// Shadertoy's BiasUV: (fragCoord + Offset) / 256.0, mapped to [-1, 1].
fn flow_bias(frag_coord: vec2f, offset: vec2f) -> vec2f {
    let bias_uv = (frag_coord + offset) / 256.0;
    return textureSampleLevel(t_noise, t_noise_sampler, bias_uv, 0.0).rg * 2.0 - 1.0;
}

@fragment fn fs_watercolor(in: VertexOutput) -> @location(0) vec4f {
    // --- Pass 0: RGB → CMYK init ---
    if (params.pass_type == 0) {
        let rgb = textureSampleLevel(t_input, t_sampler, in.uv, 0.0).rgb;
        return rgb_to_cmyk(clamp(rgb, vec3f(0.0), vec3f(1.0)));
    }

    // --- Pass 2: CMYK → RGB final ---
    if (params.pass_type == 2) {
        let cmyk = textureSampleLevel(t_input, t_sampler, in.uv, 0.0);
        return vec4f(cmyk_to_rgb(cmyk), 1.0);
    }

    // --- Pass 1: blur iteration ---
    let resolution = vec2f(params.resolution_x, params.resolution_y);
    let frag_coord = in.uv * resolution;

    let kernel = array<vec2f, 8>(
        vec2f(-1.0,  0.0),
        vec2f(-1.0, -1.0),
        vec2f( 0.0, -1.0),
        vec2f( 1.0, -1.0),
        vec2f( 1.0,  0.0),
        vec2f( 1.0,  1.0),
        vec2f( 0.0,  1.0),
        vec2f(-1.0,  1.0),
    );

    let wetness = params.wetness;

    var color = vec4f(0.0);
    for (var i = 0; i < 8; i++) {
        let offset = normalize(kernel[i]);
        let bias = flow_bias(frag_coord, offset);
        let sample_uv = (frag_coord + (offset + bias) * wetness * 5.0) / resolution;
        color += textureSampleLevel(t_input, t_sampler, sample_uv, 0.0);
    }
    color /= 8.0;

    return color;
}
