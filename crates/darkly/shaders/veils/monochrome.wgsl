// Monochrome post-processing veil.
// Converts to grayscale with configurable channel weights and optional color tint.

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
    red_weight: f32,
    green_weight: f32,
    blue_weight: f32,
    tint_hue: f32,
    tint_strength: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

/// Convert HSV to RGB. Expects h in [0,360), s and v in [0,1].
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> vec3f {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - abs(hp % 2.0 - 1.0));
    let m = v - c;

    var rgb: vec3f;
    if hp < 1.0 {
        rgb = vec3f(c, x, 0.0);
    } else if hp < 2.0 {
        rgb = vec3f(x, c, 0.0);
    } else if hp < 3.0 {
        rgb = vec3f(0.0, c, x);
    } else if hp < 4.0 {
        rgb = vec3f(0.0, x, c);
    } else if hp < 5.0 {
        rgb = vec3f(x, 0.0, c);
    } else {
        rgb = vec3f(c, 0.0, x);
    }
    return rgb + m;
}

@fragment fn fs_monochrome(in: VertexOutput) -> @location(0) vec4f {
    let color = textureSampleLevel(t_input, t_sampler, in.uv, 0.0);

    // Normalize weights so they sum to 1.0 (avoids brightness shifts).
    let w = vec3f(params.red_weight, params.green_weight, params.blue_weight);
    let w_sum = w.x + w.y + w.z;
    let w_norm = select(w / w_sum, vec3f(1.0 / 3.0), w_sum < 0.001);

    let lum = dot(color.rgb, w_norm);

    // Apply tint: convert hue to an RGB color, then mix with grayscale.
    let tint_rgb = hsv_to_rgb(params.tint_hue, 1.0, 1.0);
    let tinted = mix(vec3f(lum), lum * tint_rgb, params.tint_strength);

    return vec4f(tinted, color.a);
}
