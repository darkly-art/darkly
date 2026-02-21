// Noise overlay filter.
// Samples a pre-generated noise texture and blends it with the input
// using overlay blend mode, controlled by an amount parameter.

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

struct NoiseParams {
    amount: f32,
    resolution: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_noise: texture_2d<f32>;
@group(0) @binding(2) var t_sampler: sampler;
@group(0) @binding(3) var<uniform> params: NoiseParams;

@fragment fn fs_noise(in: VertexOutput) -> @location(0) vec4f {
    let color = textureSample(t_input, t_sampler, in.uv);

    // Sample noise texture — UV maps pixel position to noise cell
    let noise_val = textureSample(t_noise, t_sampler, in.uv).r;

    // Overlay blend mode: 2*a*b if a < 0.5, else 1 - 2*(1-a)*(1-b)
    let noise_rgb = vec3f(noise_val);
    let base = color.rgb;
    let lo = 2.0 * base * noise_rgb;
    let hi = 1.0 - 2.0 * (1.0 - base) * (1.0 - noise_rgb);
    let blended = select(hi, lo, base < vec3f(0.5));

    // Mix between original and blended by amount
    return vec4f(mix(color.rgb, blended, params.amount), color.a);
}
