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
    // Tonal contrast exponent offset — output = pow(value, 1.0 + darkness).
    // 0 = linear (washed grayscale); higher values push midtones toward
    // black for a Watery-style mood.
    darkness: f32,
    time: f32,
    // Multiplier from render-target pixel coords to canvas-space pixel
    // coords. The procedural texture renders into an aux buffer that may
    // be smaller than the canvas (typically 1/2) and is bilinear-upsampled
    // to the void's destination. Scaling here keeps the FBM domain canvas-
    // aligned regardless of render-target size, so a given `frequency`
    // produces the same feature size in the final output at any aux scale.
    canvas_scale: f32,
    _pad0: f32,
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
    // FragCoord is in render-target pixels; scale to canvas space so the
    // FBM domain is independent of aux-texture resolution.
    let xy = in.position.xy * params.canvas_scale * params.frequency;
    let p = vec3f(xy, params.time * Z_SCALE);

    let v = fbm_warp3(p, params.seed, params.octaves, LACUNARITY, GAIN, params.warp);
    // Apply darkness/contrast curve. `max` guards against the rare
    // out-of-[0,1] FBM excursion under heavy warp.
    let shaped = pow(max(v, 0.0), 1.0 + params.darkness);
    return vec4f(shaped, shaped, shaped, 1.0);
}
