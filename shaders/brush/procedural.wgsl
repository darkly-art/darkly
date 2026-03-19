// Procedural dab generation: renders an SDF circle/gaussian to a dab texture.
// Render target: RGBA8 dab texture, REPLACE blend, LoadOp::Clear(transparent).
// Viewport is set to (0, 0, dab_diameter, dab_diameter) by the host.

struct DabUniforms {
    dab_size: f32,       // actual dab diameter in pixels (matches viewport)
    radius: f32,         // SDF circle radius in pixels
    softness: f32,       // edge softness in pixels (min 1.0 for AA)
    opacity: f32,        // dab opacity (0-1)
    color: vec4f,        // RGBA paint color (straight alpha, premultiplied on output)
    rotation: f32,       // dab rotation in radians (used by stamp tips, no-op for SDF circle)
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> u: DabUniforms;

@fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let center = vec2f(u.dab_size * 0.5);
    let dist = distance(pos.xy, center);

    // SDF circle with soft edge.
    let coverage = 1.0 - smoothstep(u.radius - u.softness, u.radius, dist);

    let a = u.color.a * coverage * u.opacity;
    return vec4f(u.color.rgb * a, a); // premultiplied alpha
}

// Full-screen triangle — 3 vertices cover the viewport.
@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    return vec4f(unit * 2.0 - 1.0, 0.0, 1.0);
}
