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

// Sample a mask in its OWN plane-anchored space from a window UV: the two-hop
// `window_uv → plane → mask-local` bridge shared by every de-fused mask pass
// (apply_mask, the passthrough-group lerp). Outside the mask footprint — and
// when `mask_size` is the zero "no footprint" sentinel — it reveals (1.0),
// matching the white mask default. One definition so the mask's bounds, not
// the host's UV, decide where it has coverage.
fn sample_mask_window(
    mask_tex: texture_2d<f32>,
    samp: sampler,
    window_uv: vec2f,
    canvas_origin: vec2f,
    canvas_size: vec2f,
    mask_offset: vec2f,
    mask_size: vec2f,
) -> f32 {
    let plane = window_uv_to_plane(window_uv, canvas_origin, canvas_size);
    let mask_uv = plane_to_selection_uv(plane, mask_offset, mask_size);
    let in_bounds = all(mask_uv >= vec2f(0.0)) && all(mask_uv <= vec2f(1.0));
    let has_footprint = mask_size.x > 0.0 && mask_size.y > 0.0;
    let raw = textureSample(mask_tex, samp, mask_uv).r;
    return select(1.0, raw, has_footprint && in_bounds);
}
