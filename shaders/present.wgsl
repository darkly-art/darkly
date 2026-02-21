// Final blit: copies the accumulator to the surface.

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@fragment fn fs_present(in: VertexOutput) -> @location(0) vec4f {
    let color = textureSample(t_input, t_sampler, in.uv);
    // Premultiplied alpha → straight alpha for surface output
    let a = max(color.a, 0.001);
    return vec4f(color.rgb / a, 1.0);
}
