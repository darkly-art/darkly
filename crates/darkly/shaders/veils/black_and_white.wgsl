// Black and White post-processing veil — the sampler-based wrapper around the
// shared `bw_transform` core (`shaders/lib/black_and_white.wgsl`, prepended
// at load time by `gpu/veils/black_and_white.rs`). Alpha passes through.

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
@group(0) @binding(2) var<uniform> params: BwParams;

@fragment fn fs_black_and_white(in: VertexOutput) -> @location(0) vec4f {
    let color = textureSampleLevel(t_input, t_sampler, in.uv, 0.0);
    return vec4f(bw_transform(color.rgb, params), color.a);
}
