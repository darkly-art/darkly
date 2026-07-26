// 3D fractional Brownian motion — time-as-Z extension of the 2D core.
//
// Sampling at `(x, y, t)` instead of `(x, y)` and advancing `t` gives a field
// that is continuous in both space and time: features smoothly appear, morph,
// and dissolve at fixed canvas positions rather than rigidly translating. Used
// by the noise void's `evolution` parameter (`shaders/voids/noise.wgsl` calls
// `fbm_warp3`).
//
// DEPENDENCY: this file uses `fbm_pcg` from `fbm2d.wgsl` (the 3D value-noise
// seed fold), so consumers must concatenate `fbm2d.wgsl` AHEAD of this file.
//
// The primitive `fbm_value_noise3` is a *hardware-filtered texture lookup*
// into a 3D noise volume rather than a software PCG-hash-and-trilerp. A single
// texture sample is roughly one GPU cycle; the compute version is ~32 PCG
// hashes + 7 lerps per call. With 15 noise calls per pixel (3 fbm3 in
// fbm_warp3 × 5 octaves), the savings dominate the shader cost.
//
// Consumers must bind:
//   @group(0) @binding(1) — a 3D Rgba8Unorm noise texture, FBM_NOISE3D_DIM
//                           per side, filled with PCG-hashed random bytes.
//   @group(0) @binding(2) — a filtering sampler with Repeat addressing.
//
// Credit: texture-sampled noise primitive is inspired by nimitz's "Watery"
// (https://www.shadertoy.com/view/MssSRS) — `noise(p) = texture(channel,
// p*.01).x`, replacing per-pixel hash computation with a single
// hardware-filtered fetch. Watery itself is licensed CC BY-NC-SA 3.0; we
// re-implement the *technique* (a 3D noise volume sampled by the FBM octave
// loop) rather than reusing any code. Contact the author (twitter: @stormoid)
// for other licensing options. Domain-warp algorithm: Inigo Quilez,
// https://iquilezles.org/articles/warp/.

@group(0) @binding(1) var fbm_noise3d_tex: texture_3d<f32>;
@group(0) @binding(2) var fbm_noise3d_sampler: sampler;

const FBM_NOISE3D_DIM: f32 = 64.0;

/// 3D value noise sampled at floating-point `p`. Trilinear blend done by
/// hardware texture filtering — see comment above.
///
/// The seed shifts the texture-space sample position. With Repeat
/// addressing every offset is valid; the PCG fold ensures adjacent seeds
/// produce decorrelated samples even though they share the underlying
/// volume. `& 0x3F` keeps the offset inside one texture period to avoid
/// the modular collisions that would otherwise hit at exact multiples of
/// FBM_NOISE3D_DIM.
fn fbm_value_noise3(p: vec3f, seed: u32) -> f32 {
    let h1 = fbm_pcg(seed);
    let h2 = fbm_pcg(h1);
    let h3 = fbm_pcg(h2);
    let seed_offset = vec3f(
        f32(h1 & 0x3Fu),
        f32(h2 & 0x3Fu),
        f32(h3 & 0x3Fu),
    );
    let uvw = (p + seed_offset) / FBM_NOISE3D_DIM;
    return textureSampleLevel(fbm_noise3d_tex, fbm_noise3d_sampler, uvw, 0.0).x;
}

/// 3D fractional Brownian motion. Same octave loop as `fbm`, with `q` in 3D.
fn fbm3(p: vec3f, seed: u32, octaves: i32, lacunarity: f32, gain: f32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var norm = 0.0;
    var q = p;
    let n = max(octaves, 1);
    for (var i = 0; i < n; i = i + 1) {
        sum = sum + amp * fbm_value_noise3(q, seed + u32(i) * 1013u);
        norm = norm + amp;
        q = q * lacunarity;
        amp = amp * gain;
    }
    return sum / norm;
}

/// 3D domain warp offset — same Quilez warp as `fbm_warp_offset`, but the
/// underlying FBM is sampled in 3D so the displacement field itself evolves
/// continuously as `p.z` advances. Returns a 2D offset; the warp is a
/// planar displacement (we don't displace `z`, which would warp time).
fn fbm_warp3_offset(
    p: vec3f,
    seed: u32,
    octaves: i32,
    lacunarity: f32,
    gain: f32,
    warp_strength: f32,
) -> vec2f {
    if (warp_strength <= 0.0) {
        return vec2f(0.0);
    }
    let qx = fbm3(p, seed + 1u, octaves, lacunarity, gain);
    let qy = fbm3(p + vec3f(5.2, 1.3, 0.0), seed + 17u, octaves, lacunarity, gain);
    return warp_strength * vec2f(qx - 0.5, qy - 0.5);
}

/// 3D domain-warped FBM scalar. The detail FBM is sampled at the warped
/// xy position; the time component `p.z` passes through unchanged so that
/// detail and warp share the same temporal cadence.
fn fbm_warp3(
    p: vec3f,
    seed: u32,
    octaves: i32,
    lacunarity: f32,
    gain: f32,
    warp_strength: f32,
) -> f32 {
    let offset = fbm_warp3_offset(p, seed, octaves, lacunarity, gain, warp_strength);
    let q = p + vec3f(offset, 0.0);
    return fbm3(q, seed + 31u, octaves, lacunarity, gain);
}
