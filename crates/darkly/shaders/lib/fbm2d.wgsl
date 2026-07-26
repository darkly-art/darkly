// 2D fractional Brownian motion core — shared, binding-free GPU helpers.
//
// The interpolated value-noise primitive, the octave loop, domain warp, and
// the per-octave-rotated variant used by the brush `noise` node. No `@group`
// declarations, so this file is safe to concatenate into ANY shader via Rust's
// `include_str!` (WGSL has no native #include). The 3D texture-sampled variants
// live in `fbm.wgsl`, which depends on `fbm_pcg` from here and must be
// concatenated AFTER this file.
//
// Credits:
//
// • Domain-warp algorithm: Inigo Quilez, "Domain warping",
//   https://iquilezles.org/articles/warp/
//
// • Per-octave rotation in `fbm_rot`: standard practice for breaking the
//   axis-aligned value-noise lattice — Inigo Quilez, "fbm",
//   https://iquilezles.org/articles/fbm/. The specific seed transform and
//   rotate-and-translate constants mirror the design prototype in
//   docs/plans/noise-preview.html.

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

/// Seed → (base rotation angle, base 2D offset), packed `(angle, ox, oy)`.
/// PCG-hashed so adjacent seeds give uncorrelated fields rather than shifted
/// copies. Any CPU preview must mirror these exact constants.
fn fbm_seed_xform(seed: u32) -> vec3f {
    let a = f32(fbm_pcg(seed)) / 4294967295.0 * 6.28318530718;
    let ox = f32(fbm_pcg(seed + 101u)) / 4294967295.0 * 64.0;
    let oy = f32(fbm_pcg(seed + 202u)) / 4294967295.0 * 64.0;
    return vec3f(a, ox, oy);
}

/// Per-octave rotated, domain-warped fBm of value noise. Each octave's sample
/// coordinate is rotated by a seed-derived base angle plus `i*0.9` and
/// translated, decorrelating the octaves' lattices so the axis-aligned
/// value-noise grid never lines up across scales — the artifact plain `fbm`
/// retains. A single-octave value-noise domain warp is applied once up front.
/// Lacunarity is fixed at 2.0; `gain` and `warp` are caller-controlled.
/// Output is roughly [0, 1].
fn fbm_rot(p: vec2f, seed: u32, octaves: i32, gain: f32, warp: f32) -> f32 {
    let xf = fbm_seed_xform(seed);
    var c = p;
    if (warp > 0.0) {
        let wx = fbm_value_noise(c + vec2f(11.5, 3.7), seed);
        let wy = fbm_value_noise(c + vec2f(5.2, 1.3), seed);
        c = c + warp * vec2f(wx - 0.5, wy - 0.5);
    }
    var sum = 0.0;
    var amp = 1.0;
    var freq = 1.0;
    var norm = 0.0;
    let n = max(octaves, 1);
    for (var i = 0; i < n; i = i + 1) {
        let ang = xf.x + f32(i) * 0.9;
        let ca = cos(ang);
        let sa = sin(ang);
        let s = c * freq;
        let r = vec2f(
            s.x * ca - s.y * sa + xf.y + f32(i) * 13.7,
            s.x * sa + s.y * ca + xf.z + f32(i) * 7.1,
        );
        sum = sum + amp * fbm_value_noise(r, seed + u32(i) * 1013u);
        norm = norm + amp;
        freq = freq * 2.0;
        amp = amp * gain;
    }
    return sum / norm;
}
