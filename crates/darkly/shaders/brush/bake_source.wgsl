// Bake a procedural noise tile by running the brush noise field (`fbm_tile`)
// once per texel into a cached texture. The compiled brush then samples that
// texture with a single `textureSample` instead of re-evaluating the
// ~80-hash domain-warped fBm kernel per fragment per overlapping dab.
//
// `shaders/lib/fbm2d.wgsl` — which owns the fBm math (`fbm_tile`,
// `fbm_value_noise`, `fbm_seed_xform`, `fbm_pcg`) — is concatenated ahead of
// this file at pipeline-build time (WGSL has no `#include`), exactly as the
// void noise pass assembles its shader. The field is NOT reimplemented here;
// this shader only maps each texel to a field coordinate and writes the
// result. Credit for the fBm/value-noise/domain-warp math is in fbm2d.wgsl
// (Inigo Quilez).

struct BakeParams {
    seed: u32,
    octaves: i32,
    gain: f32,
    warp: f32,
    // Field units the tile spans across [0,1) uv — matches the node's
    // sample-time divide by TILE_SPAN so baked feature size equals the live
    // path and the field repeats once per this many field units.
    tile_span: f32,
    // 0 = grayscale value (single fBm at the base seed, written to R),
    // 1 = chromatic rgba (three fBm at seed+{0,1,2}).
    channels: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<uniform> params: BakeParams;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Full-screen triangle; uv spans [0,1) across the framebuffer.
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let p = corners[vi];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = (p + vec2<f32>(1.0, 1.0)) * 0.5;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The tile spans `tile_span` field units across [0,1) uv; `fbm_tile` wraps
    // its lattice at that same period, so the baked tile is seamless under the
    // repeat-wrapped sampler.
    let coord = in.uv * params.tile_span;
    let period = i32(params.tile_span);
    if (params.channels == 0u) {
        let v = fbm_tile(coord, params.seed, params.octaves, params.gain, params.warp, period);
        return vec4<f32>(v, v, v, 1.0);
    }
    // Three independent channels off consecutive seeds — identical seed math
    // to the node's live chromatic path (`CHANNEL_SEED_OFFSETS = [0, 1, 2]`).
    let r = fbm_tile(coord, params.seed, params.octaves, params.gain, params.warp, period);
    let g = fbm_tile(coord, params.seed + 1u, params.octaves, params.gain, params.warp, period);
    let b = fbm_tile(coord, params.seed + 2u, params.octaves, params.gain, params.warp, period);
    return vec4<f32>(r, g, b, 1.0);
}
