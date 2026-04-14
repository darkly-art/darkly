// Circle node: renders an SDF circle mask to a dab texture.
//
// Produces a white circle with soft edges — a grayscale alpha mask.
// The stamp node handles sizing, color, rotation, and compositing.
// Viewport is always MAX_DAB_SIZE × MAX_DAB_SIZE (full pool texture).

struct CircleUniforms {
    softness: f32,   // 0-1 fraction of radius (0 = hard edge, 1 = fully soft)
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> u: CircleUniforms;

@fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let size = 512.0; // MAX_DAB_SIZE
    let center = vec2f(size * 0.5);
    let radius = size * 0.5 - 1.0; // 1px margin for AA
    let softness_px = max(u.softness * radius, 1.0);
    let dist = distance(pos.xy, center);
    let coverage = 1.0 - smoothstep(radius - softness_px, radius, dist);
    return vec4f(coverage, coverage, coverage, coverage);
}

// Full-screen triangle — 3 vertices cover the viewport.
@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    return vec4f(unit * 2.0 - 1.0, 0.0, 1.0);
}
