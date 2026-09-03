// Chromatic aberration post-processing veil, the whole-canvas fringe at render
// resolution. The shared per-pixel math lives in `lib/aberration.wgsl`, which
// gpu/veils/chromatic_aberration.rs prepends at load time (WGSL has no #include).
//
// Offsets/blur are render-target pixels, so they scale with
// `rendering.veil_scale`, the same convention as other pixel-domain veils.

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
@group(0) @binding(2) var<uniform> params: AberrationParams;

@fragment fn fs_chromatic_aberration(in: VertexOutput) -> @location(0) vec4f {
    return aberration_apply(t_input, t_sampler, in.uv, params);
}
