// Frozen glass post-processing veil.
// Refracts the scene through a static ice normal map: the XY components
// of the normal offset the scene UV so the image appears distorted as
// if viewed through frozen glass.

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
    resolution_x: f32,
    resolution_y: f32,
    normal_aspect: f32,
    strength: f32,
    scale: f32,
    chromatic: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var t_normal: texture_2d<f32>;
@group(0) @binding(4) var t_normal_sampler: sampler;

/// Map a screen-space UV to the normal map, preserving the normal map's
/// aspect ratio so the ice pattern never stretches. The normal map tiles
/// via REPEAT addressing; `scale` controls tile density (1.0 = one tile
/// across the smaller screen dimension).
fn normal_uv(screen_uv: vec2f) -> vec2f {
    let screen_aspect = params.resolution_x / params.resolution_y;
    // Centered coords in [-0.5, 0.5] scaled by aspect so one "unit"
    // is the same in X and Y.
    var p = (screen_uv - 0.5) * vec2f(screen_aspect, 1.0) / params.scale;
    // Re-compensate for the normal map's own aspect ratio so a square
    // region of screen samples a square region of the normal map.
    p.x /= params.normal_aspect;
    return p + 0.5;
}

@fragment fn fs_frozen(in: VertexOutput) -> @location(0) vec4f {
    let n_uv = normal_uv(in.uv);
    // Normal map stores (n*0.5+0.5); decode to [-1, 1].
    // DirectX convention: +G = down. Y is flipped to match our screen UV
    // (which also runs top-down), so the refraction direction matches the
    // visible bumps on the normal map.
    let sample = textureSample(t_normal, t_normal_sampler, n_uv).rgb;
    let n = sample * 2.0 - 1.0;

    // Aspect-corrected UV displacement so the refraction looks isotropic
    // regardless of viewport shape.
    let screen_aspect = params.resolution_x / params.resolution_y;
    let disp = vec2f(n.x / screen_aspect, n.y) * params.strength;

    // Chromatic aberration: per-channel displacement scaled slightly
    // apart so the glass has a subtle prism edge. 0 = clean refraction.
    let ca = params.chromatic;
    let r_uv = in.uv + disp * (1.0 + ca);
    let g_uv = in.uv + disp;
    let b_uv = in.uv + disp * (1.0 - ca);

    let r = textureSample(t_input, t_sampler, r_uv).r;
    let g = textureSample(t_input, t_sampler, g_uv).g;
    let b = textureSample(t_input, t_sampler, b_uv).b;

    return vec4f(r, g, b, 1.0);
}
