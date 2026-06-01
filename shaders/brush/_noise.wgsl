// 2D cell-noise prelude — shared helpers for compiled brush graphs that
// sample procedural noise per fragment (paper grain, scatter masks, etc.).
//
// Single 2D `node_noise_value(p, seed) -> [0, 1)` that returns the hash
// of `floor(p)` directly — no bilinear blend, no fade. Every cell is
// independent of its neighbours, so adjacent samples that don't share
// a cell are uncorrelated. That's the "every pixel is a random value"
// shape, with `scale` (caller-side, `p = target_pos / scale`) deciding
// how many pixels share a cell.
//
// Symbols are prefixed `node_noise_` to keep them out of the `fbm_`
// namespace the void prelude uses — both prefixes coexist in the same
// process but compile into different shader programs, so the prefix is
// purely a stylistic guard against future cross-pollination if someone
// later #includes both into the same WGSL.
//
// Credits:
//   - Integer PCG hash: M.E. O'Neill, "PCG, A Family of Simple Fast
//     Space-Efficient Statistically Good Algorithms for Random Number
//     Generation" (2014).

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

/// 2D cell noise sampled at floating-point `p`. Returns the hash of the
/// surrounding integer cell — no interpolation between cells. Output is
/// roughly uniform in [0, 1).
fn node_noise_value(p: vec2<f32>, seed: u32) -> f32 {
    return node_noise_hash2(vec2<i32>(floor(p)), seed);
}
