// Canvas-window coordinate helpers.
//
// Darkly has one coordinate frame — canvas/plane space — in which layer
// extents, paint positions, and selection regions all live. The visible
// "canvas window" is a rectangle `(canvas_origin, canvas_size)` within that
// plane (moved by crop / resize). The selection mask is a *window-sized* R8
// texture anchored at `canvas_origin`, so sampling it from a plane position
// requires subtracting the window origin first.
//
// One definition, reused by every shader that samples the selection mask
// (paint_circle, gradient, brush composite, the brush node-graph codegen) and
// by the layer compositor (window UV → plane position). Concatenated ahead of
// each shader at module-creation time — WGSL has no `#include`.

// Plane position `p` → selection-mask UV. The mask is window-sized at
// `origin`, so `(p - origin) / size` lands plane pixels on the right texels.
fn plane_to_selection_uv(p: vec2f, origin: vec2f, size: vec2f) -> vec2f {
    return (p - origin) / size;
}

// Window UV (0..1 across the accumulator) → plane position. Inverse of the
// above for the layer compositor, which rasterizes the window but samples
// plane-anchored layer textures.
fn window_uv_to_plane(uv: vec2f, origin: vec2f, size: vec2f) -> vec2f {
    return origin + uv * size;
}
