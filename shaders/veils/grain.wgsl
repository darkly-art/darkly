// Grain veil — per-pixel noise with gradual evolution, applied via overlay blend.
//
// Two-pass architecture:
//   1. Evolve pass (fs_evolve): blends previous noise state toward fresh random
//      noise by `rate` per frame, maintaining a persistent noise texture.
//   2. Apply pass (fs_apply): reads the evolved noise texture and overlay-blends
//      it onto the scene image, mixed with the original by `opacity`.

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

struct Params {
    seed: f32,
    color: f32,
    rate: f32,
    opacity: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var t_noise_state: texture_2d<f32>;

/// Integer hash — PCG-style. Fast, high quality, no visible patterns.
fn pcg(n: u32) -> u32 {
    var h = n * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

/// Hash a 2D pixel coordinate + seed into a uniform float in [0, 1).
fn hash_pixel(coord: vec2u, seed: u32) -> f32 {
    let h = pcg(coord.x + pcg(coord.y + pcg(seed)));
    return f32(h) / 4294967295.0;
}

/// Generate 3 independent noise values for RGB channels.
fn hash_pixel_rgb(coord: vec2u, seed: u32) -> vec3f {
    let s0 = pcg(seed);
    let s1 = pcg(s0);
    let s2 = pcg(s1);
    return vec3f(
        hash_pixel(coord, s0),
        hash_pixel(coord, s1),
        hash_pixel(coord, s2),
    );
}

/// Photoshop overlay blend: 2*a*b if a < 0.5, else 1 - 2*(1-a)*(1-b).
fn overlay_blend(base: vec3f, blend: vec3f) -> vec3f {
    let lo = 2.0 * base * blend;
    let hi = 1.0 - 2.0 * (1.0 - base) * (1.0 - blend);
    return select(hi, lo, base < vec3f(0.5));
}

/// Evolve pass: randomly replace a fraction of pixels with fresh noise.
/// Each pixel independently flips a coin — if the hash falls below `rate`,
/// the pixel gets new random noise; otherwise it keeps its previous value.
/// This preserves full noise variance at all evolution rates (no averaging).
/// Binding 0 (t_input) is bound to the previous noise state texture.
@fragment fn fs_evolve(in: VertexOutput) -> @location(0) vec4f {
    let prev = textureSampleLevel(t_input, t_sampler, in.uv, 0.0);
    let dims = textureDimensions(t_input, 0);
    let coord = vec2u(in.uv * vec2f(dims));
    let seed = u32(params.seed);
    let rgb = hash_pixel_rgb(coord, seed);
    let gray = hash_pixel(coord, seed);
    let fresh = vec4f(rgb, gray);

    // Per-pixel coin flip: replace this pixel only if hash < rate.
    let coin = hash_pixel(coord, seed + 12345u);
    let replace = step(coin, params.rate);
    return mix(prev, fresh, replace);
}

/// Apply pass: overlay-blend evolved noise onto the scene image, then mix
/// with the original by `opacity` (1 = full grain, 0 = veil is a no-op).
/// Binding 0 (t_input) is the scene, binding 3 (t_noise_state) is evolved noise.
@fragment fn fs_apply(in: VertexOutput) -> @location(0) vec4f {
    let color = textureSampleLevel(t_input, t_sampler, in.uv, 0.0);
    let evolved = textureSampleLevel(t_noise_state, t_sampler, in.uv, 0.0);
    let noise = mix(vec3f(evolved.a), evolved.rgb, params.color);
    let overlaid = overlay_blend(color.rgb, noise);
    let result = mix(color.rgb, overlaid, params.opacity);
    return vec4f(result, color.a);
}
