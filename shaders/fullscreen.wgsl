// Fullscreen triangle vertex shader — no vertex buffer needed.
// Draw with 3 vertices, the triangle covers the entire screen.

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    // Flip Y so UV (0,0) is top-left
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}
