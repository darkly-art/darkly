// 2D value-noise prelude — shared helpers for compiled brush graphs that
// sample procedural noise per fragment (paper grain, scatter masks, etc.).
//
// Single 2D `node_noise_value(p, seed) -> [0, 1)` plus the integer
// hash + quintic fade it composes from. Lifted from the FBM library at
// `shaders/lib/fbm.wgsl` (used by the void / veil layers) and trimmed to
// what brushes actually need: a single octave at the call site, octave
// stacking left to the consumer if it wants fBm.
//
// Symbols are prefixed `node_noise_` to keep them out of the `fbm_`
// namespace the void prelude uses — both prefixes coexist in the same
// process but compile into different shader programs, so the prefix is
// purely a stylistic guard against future cross-pollination if someone
// later #includes both into the same WGSL.
//
// Credits — same lineage as `shaders/lib/fbm.wgsl`:
//   - Integer PCG hash: M.E. O'Neill, "PCG, A Family of Simple Fast
//     Space-Efficient Statistically Good Algorithms for Random Number
//     Generation" (2014).
//   - Quintic fade + value-noise structure: Ken Perlin; Inigo Quilez
//     (https://iquilezles.org/articles/morenoise/).

fn node_noise_pcg(n: u32) -> u32 {
    var h = n * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

/// Hash an integer 2D coordinate plus a seed into a uniform float in [0, 1).
fn node_noise_hash2(coord: vec2<i32>, seed: u32) -> f32 {
    let cx = bitcast<u32>(coord.x);
    let cy = bitcast<u32>(coord.y);
    let h = node_noise_pcg(cx + node_noise_pcg(cy + node_noise_pcg(seed)));
    return f32(h) / 4294967295.0;
}

/// Quintic smoothstep — C2-continuous, avoids the directional banding
/// cubic smoothstep produces.
fn node_noise_fade(t: vec2<f32>) -> vec2<f32> {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

/// 2D value noise sampled at floating-point `p`. Bilinear blend of the
/// four surrounding integer-cell hashes through the quintic fade.
/// Returns a scalar in roughly [0, 1].
fn node_noise_value(p: vec2<f32>, seed: u32) -> f32 {
    let pi = vec2<i32>(floor(p));
    let pf = fract(p);
    let w = node_noise_fade(pf);
    let a = node_noise_hash2(pi + vec2<i32>(0, 0), seed);
    let b = node_noise_hash2(pi + vec2<i32>(1, 0), seed);
    let c = node_noise_hash2(pi + vec2<i32>(0, 1), seed);
    let d = node_noise_hash2(pi + vec2<i32>(1, 1), seed);
    let ab = mix(a, b, w.x);
    let cd = mix(c, d, w.x);
    return mix(ab, cd, w.y);
}
