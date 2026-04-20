// Circle node: renders an SDF circle mask to a dab texture.
//
// Produces a white circle with soft edges — a grayscale alpha mask that
// fills the current viewport. The caller picks the viewport size (the
// dab's canvas-pixel extent); the shader draws a unit-circle mask scaled
// into that viewport. Works at any viewport size, not just MAX_DAB_SIZE.
//
// Soft edges: `softness` is the fraction of the radius (0..1) over which
// coverage fades from 1 (inside) to 0 (outside). 0 = hard edge, 1 = fully
// feathered.

struct CircleUniforms {
    softness: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> u: CircleUniforms;

struct VertexOutput {
    @builtin(position) position: vec4f,
    // UV in [0, 1] across the viewport — enables viewport-size-agnostic
    // SDF evaluation (compare distance against 0.5 instead of pixel radii).
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle (3 verts): UV corners are (0,0), (2,0), (0,2).
    // The interpolated uv inside the viewport quadrant (0..1) is what we
    // sample; the far-corner region outside the quad is clipped away.
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VertexOutput;
    out.position = vec4f(unit * 2.0 - 1.0, 0.0, 1.0);
    out.uv = unit;
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // SDF space: viewport centred at (0.5, 0.5), outer radius 0.5. Softness
    // is a fraction of that radius — 0 = hard 1-px edge, 1 = fully feathered
    // from the centre. Multiplying `radius` by the viewport's pixel count
    // would give a sub-pixel AA band; we approximate with a viewport-space
    // 1/N band where N is large enough to avoid visible banding.
    let center = vec2f(0.5, 0.5);
    let radius = 0.5;
    // Keep a tiny outer margin to ensure anti-aliased edges land inside the
    // viewport regardless of its pixel size.
    let outer = radius - 0.002;
    let softness_band = max(u.softness, 0.004) * radius;
    let dist = distance(in.uv, center);
    let coverage = 1.0 - smoothstep(outer - softness_band, outer, dist);
    return vec4f(coverage, coverage, coverage, coverage);
}
