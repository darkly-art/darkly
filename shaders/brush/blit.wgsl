// Simple UV-windowed blit: sample a sub-rectangle of a source texture and
// stretch it across the entire viewport.
//
// Uses for KIND_MASKED_STAMP preview (preview_output node): the source is
// the dab_pool texture (512×512) with content in a small corner; we want
// to sample only (0..dab_w/512, 0..dab_h/512) and fill the preview target.

struct BlitUniforms {
    uv_min: vec2f,
    uv_max: vec2f,
}

@group(0) @binding(0) var<uniform> u: BlitUniforms;
@group(1) @binding(0) var t_src: texture_2d<f32>;
@group(1) @binding(1) var s_src: sampler;

struct VertexOutput {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle: vertex indices 0,1,2 cover the viewport.
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VertexOutput;
    // Flip Y in NDC so UV (0,0) lands on the top-left dest texel — matches
    // source texture orientation (UV origin = top-left in WGPU). Without
    // this flip the dest is vertically mirrored vs the source.
    out.pos = vec4f(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
    out.uv = mix(u.uv_min, u.uv_max, unit);
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return textureSample(t_src, s_src, in.uv);
}
