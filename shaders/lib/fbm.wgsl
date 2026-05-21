// Domain-warped fractional Brownian motion (FBM) — shared GPU helper.
//
// Cloud / marble / lightning textures with infinite seed-driven variation.
// The void layer (`shaders/voids/noise.wgsl`) consumes `fbm_warp` directly as
// a scalar field; a future warp veil will consume `fbm_warp_offset` as a 2D
// displacement map. Same primitives, different output contract.
//
// This file declares functions only — no @group / @binding / entry points.
// Consumers concatenate it ahead of their own WGSL via Rust's `include_str!`
// (WGSL has no native #include).
//
// Algorithm based on Inigo Quilez's domain-warping article:
//   https://iquilezles.org/articles/warp/
// Value-noise primitive uses a PCG-style integer hash for cheap, pattern-
// free pseudo-random per-cell values.

/// Integer PCG hash. Fast, well-distributed, no visible patterns.
fn fbm_pcg(n: u32) -> u32 {
    var h = n * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

/// Hash an integer 2D coordinate plus a seed into a uniform float in [0, 1).
fn fbm_hash2(coord: vec2i, seed: u32) -> f32 {
    let cx = bitcast<u32>(coord.x);
    let cy = bitcast<u32>(coord.y);
    let h = fbm_pcg(cx + fbm_pcg(cy + fbm_pcg(seed)));
    return f32(h) / 4294967295.0;
}

/// Quintic smoothstep — Perlin's improved fade. C2-continuous, avoids the
/// directional banding cubic smoothstep produces in stacked octaves.
fn fbm_fade(t: vec2f) -> vec2f {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

/// 2D value noise sampled at floating-point `p`. Bilinear blend of the
/// four surrounding integer-cell hashes through `fbm_fade`. Returns a
/// scalar in roughly [0, 1].
fn fbm_value_noise(p: vec2f, seed: u32) -> f32 {
    let pi = vec2i(floor(p));
    let pf = fract(p);
    let w = fbm_fade(pf);
    let a = fbm_hash2(pi + vec2i(0, 0), seed);
    let b = fbm_hash2(pi + vec2i(1, 0), seed);
    let c = fbm_hash2(pi + vec2i(0, 1), seed);
    let d = fbm_hash2(pi + vec2i(1, 1), seed);
    let ab = mix(a, b, w.x);
    let cd = mix(c, d, w.x);
    return mix(ab, cd, w.y);
}

/// Fractional Brownian motion — sum `octaves` octaves of value noise with
/// per-octave frequency scaled by `lacunarity` and amplitude scaled by
/// `gain`. Output is renormalized to roughly [0, 1] regardless of gain.
fn fbm(p: vec2f, seed: u32, octaves: i32, lacunarity: f32, gain: f32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var freq = 1.0;
    var norm = 0.0;
    var q = p;
    let n = max(octaves, 1);
    for (var i = 0; i < n; i = i + 1) {
        // Offset each octave's seed so they sample uncorrelated fields —
        // otherwise low frequencies and high frequencies would peak at the
        // same world-space coordinates and the FBM would look like a single
        // smoothed copy of itself instead of layered detail.
        sum = sum + amp * fbm_value_noise(q, seed + u32(i) * 1013u);
        norm = norm + amp;
        q = q * lacunarity;
        amp = amp * gain;
        freq = freq * lacunarity;
    }
    return sum / norm;
}

/// 2D domain warp offset — Quilez's two-stage warp. Sample two independent
/// FBM fields, treat them as (x, y) of a displacement vector. The void
/// shader adds this to its base sample point; a future displacement-warp
/// veil will use this directly to perturb the underlying composite.
///
/// `warp_strength = 0` returns `vec2f(0.0)`, so callers can dial warp
/// continuously from "pure FBM" to "fully marbled".
fn fbm_warp_offset(
    p: vec2f,
    seed: u32,
    octaves: i32,
    lacunarity: f32,
    gain: f32,
    warp_strength: f32,
) -> vec2f {
    if (warp_strength <= 0.0) {
        return vec2f(0.0);
    }
    // Two FBM fields, sampled with independent seed offsets so the x and y
    // components of the displacement are uncorrelated.
    let qx = fbm(p, seed + 1u, octaves, lacunarity, gain);
    let qy = fbm(p + vec2f(5.2, 1.3), seed + 17u, octaves, lacunarity, gain);
    // Center the [0,1] FBM output around zero so the warp is symmetric.
    return warp_strength * vec2f(qx - 0.5, qy - 0.5);
}

/// Domain-warped FBM scalar. Computes the warp offset, adds it to `p`, then
/// samples a fresh FBM field at the warped position. Output is in roughly
/// [0, 1] — same range as `fbm` itself, so callers can mix freely.
fn fbm_warp(
    p: vec2f,
    seed: u32,
    octaves: i32,
    lacunarity: f32,
    gain: f32,
    warp_strength: f32,
) -> f32 {
    let q = p + fbm_warp_offset(p, seed, octaves, lacunarity, gain, warp_strength);
    return fbm(q, seed + 31u, octaves, lacunarity, gain);
}

// =========================================================================
// 3D variants — time-as-Z extension of the 2D helpers above.
//
// Sampling at `(x, y, t)` instead of `(x, y)` and advancing `t` gives a
// field that is continuous in both space and time: features smoothly
// appear, morph, and dissolve at fixed canvas positions rather than
// rigidly translating. Used by the noise void's `evolution` parameter.
// =========================================================================

/// Hash an integer 3D coordinate plus a seed into a uniform float in [0, 1).
fn fbm_hash3(coord: vec3i, seed: u32) -> f32 {
    let cx = bitcast<u32>(coord.x);
    let cy = bitcast<u32>(coord.y);
    let cz = bitcast<u32>(coord.z);
    let h = fbm_pcg(cx + fbm_pcg(cy + fbm_pcg(cz + fbm_pcg(seed))));
    return f32(h) / 4294967295.0;
}

/// Quintic fade for three components.
fn fbm_fade3(t: vec3f) -> vec3f {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

/// 3D value noise sampled at floating-point `p`. Trilinear blend of the
/// eight surrounding integer-cell hashes through `fbm_fade3`.
fn fbm_value_noise3(p: vec3f, seed: u32) -> f32 {
    let pi = vec3i(floor(p));
    let pf = fract(p);
    let w = fbm_fade3(pf);
    let c000 = fbm_hash3(pi + vec3i(0, 0, 0), seed);
    let c100 = fbm_hash3(pi + vec3i(1, 0, 0), seed);
    let c010 = fbm_hash3(pi + vec3i(0, 1, 0), seed);
    let c110 = fbm_hash3(pi + vec3i(1, 1, 0), seed);
    let c001 = fbm_hash3(pi + vec3i(0, 0, 1), seed);
    let c101 = fbm_hash3(pi + vec3i(1, 0, 1), seed);
    let c011 = fbm_hash3(pi + vec3i(0, 1, 1), seed);
    let c111 = fbm_hash3(pi + vec3i(1, 1, 1), seed);
    let x00 = mix(c000, c100, w.x);
    let x10 = mix(c010, c110, w.x);
    let x01 = mix(c001, c101, w.x);
    let x11 = mix(c011, c111, w.x);
    let y0 = mix(x00, x10, w.y);
    let y1 = mix(x01, x11, w.y);
    return mix(y0, y1, w.z);
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
