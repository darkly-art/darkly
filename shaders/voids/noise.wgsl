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
// Time advances the z-axis of a 3D FBM field. Features morph in place at
// fixed canvas positions rather than translating; at `evolution = 1.0` and
// a 60 Hz void clock, one z-cell-cross takes ~7 seconds — visible but not
// frenetic. Tune to taste.
const Z_SCALE: f32 = 0.15;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // FragCoord is in pixels — no resolution uniform needed; the shader works
    // at whatever target size the compositor allocated.
    let xy = in.position.xy * params.frequency;
    let p = vec3f(xy, params.time * Z_SCALE);

    let v = fbm_warp3(p, params.seed, params.octaves, LACUNARITY, GAIN, params.warp);

    // Color path: three independent FBM fields at offset seeds give the RGB
    // channels uncorrelated structure. `params.color` lerps from a grayscale
    // ramp (single field replicated) to full RGB. xy-only offsets keep all
    // three channels phase-locked in time.
    let g = fbm_warp3(p + vec3f(7.3, 2.1, 0.0), params.seed + 137u, params.octaves, LACUNARITY, GAIN, params.warp);
    let b = fbm_warp3(p + vec3f(-3.7, 9.5, 0.0), params.seed + 271u, params.octaves, LACUNARITY, GAIN, params.warp);
    let col = mix(vec3f(v), vec3f(v, g, b), params.color);

    return vec4f(col, 1.0);
}
