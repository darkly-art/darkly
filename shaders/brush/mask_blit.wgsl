// Format-bridging blit passes used by `GpuPaintTarget`'s brush methods.
//
// `fs_broadcast_r`: sample a single-channel (R8) source and broadcast its
//   value into the destination as `(r, r, r, 1)`. Used to populate the
//   brush's RGBA8 pre-stroke snapshot when the layer is an R8 mask.
// `fs_passthrough`: sample an RGBA8 source unchanged. Used by liquify's
//   commit when writing scratch into an R8 mask destination — the GPU
//   silently drops G/B/A on the R8 target so this becomes the "extract
//   .r" path with no shader-side reduction.
//
// Single fullscreen triangle vertex shader — no UV uniforms; the source
// and destination are assumed to share dimensions and orientation.

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;

struct VertexOutput {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle: indices 0,1,2 cover the viewport.
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VertexOutput;
    // Flip Y so UV (0,0) lands on top-left of the destination, matching
    // the source texture orientation (UV origin = top-left in WGPU).
    out.pos = vec4f(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
    out.uv = vec2f(unit.x, unit.y);
    return out;
}

@fragment fn fs_broadcast_r(in: VertexOutput) -> @location(0) vec4f {
    let r = textureSample(t_src, s_src, in.uv).r;
    return vec4f(r, r, r, 1.0);
}

@fragment fn fs_passthrough(in: VertexOutput) -> @location(0) vec4f {
    return textureSample(t_src, s_src, in.uv);
}
