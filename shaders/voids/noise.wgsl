// Noise void — domain-warped FBM rendered into the layer's color buffer.
//
// The FBM primitive functions (`fbm`, `fbm_warp`, `fbm_warp_offset`, helpers)
// live in `shaders/lib/fbm.wgsl` and are concatenated ahead of this file at
// pipeline-creation time. A future warp veil will reuse the same helpers as a
// displacement map instead of a texture.

struct VertexOutput {
    @builtin(position) position: vec4f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

struct Params {
    seed: u32,
    octaves: i32,
    frequency: f32,
    warp: f32,
    color: f32,
    time: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<uniform> params: Params;

const LACUNARITY: f32 = 2.0;
const GAIN: f32 = 0.5;
// Time drifts the sample point at a fixed, mildly off-axis rate so the field
// scrolls cleanly without aligning to either pixel axis.
const DRIFT: vec2f = vec2f(0.5, 0.31);

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // FragCoord is in pixels — no resolution uniform needed; the shader works
    // at whatever target size the compositor allocated.
    let pixel = in.position.xy + params.time * DRIFT;
    let p = pixel * params.frequency;

    let v = fbm_warp(p, params.seed, params.octaves, LACUNARITY, GAIN, params.warp);

    // Color path: three independent FBM fields at offset seeds give the RGB
    // channels uncorrelated structure. `params.color` lerps from a grayscale
    // ramp (single field replicated) to full RGB.
    let g = fbm_warp(p + vec2f(7.3, 2.1), params.seed + 137u, params.octaves, LACUNARITY, GAIN, params.warp);
    let b = fbm_warp(p + vec2f(-3.7, 9.5), params.seed + 271u, params.octaves, LACUNARITY, GAIN, params.warp);
    let col = mix(vec3f(v), vec3f(v, g, b), params.color);

    return vec4f(col, 1.0);
}
