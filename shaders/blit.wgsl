// Passthrough blit shader.
// Samples input texture and writes it unchanged.
// Used by veils for resolution-based effects (downscale/upscale)
// and by the compositor for the final veil→surface blit.

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

@fragment fn fs_blit(in: VertexOutput) -> @location(0) vec4f {
    return textureSample(t_input, t_sampler, in.uv);
}
