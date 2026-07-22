// Destructive mask bake: multiply a host layer's alpha by a mask sampled in
// the mask's OWN plane-anchored frame, so "what you bake" is definitionally
// "what you saw" — the live composite modulates alpha through the same
// `sample_mask_plane` footprint logic (shaders/apply_mask.wgsl).
//
// Keeps paint_circle's extent-aware vertex stage (quad positioned via
// origin/size, NDC via target_offset/target_size, so it writes in place over
// the host texture's own extent) but replaces the fragment with a
// footprint-aware alpha-multiply: outputs (0, 0, 0, mask) so the alpha-only
// multiply blend does `dst.a *= mask`, `dst.rgb` unchanged. Outside the mask
// footprint the mask reveals 1.0 — host alpha is preserved, matching the
// display path (paint_circle's selection default would smear the edge texel).

struct Uniforms {
    // Quad origin in canvas pixels (top-left corner).
    origin: vec2f,
    // Quad size in canvas pixels.
    size: vec2f,
    // Canvas-space offset of the target's (0,0) pixel.
    target_offset: vec2f,
    // Target texture pixel dimensions (used for vertex NDC mapping).
    target_size: vec2f,
    // Document canvas size (unused here; kept for PaintUniforms layout parity).
    canvas_size: vec2f,
    // Plane-space offset of the canvas window (unused here; layout parity).
    canvas_origin: vec2f,
    // Circle center (unused here; layout parity).
    center: vec2f,
    // Circle radius (unused here; layout parity).
    radius: f32,
    // Soft edge width (unused here; layout parity).
    softness: f32,
    // Paint color (unused here; layout parity).
    color: vec4f,
    // Mask texture's plane-space offset (top-left) and pixel size. A zero
    // `mask_size` is the "no footprint" sentinel (reveal everywhere).
    mask_offset: vec2f,
    mask_size: vec2f,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var t_selection: texture_2d<f32>;
@group(1) @binding(1) var t_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle trick: 3 vertices cover the [0,1]² square.
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));

    // Map unit quad to the paint region's canvas-space rectangle.
    let canvas_pos = uniforms.origin + unit * uniforms.size;

    // Translate canvas-space → target-local, then to NDC against target size.
    let target_local = canvas_pos - uniforms.target_offset;
    let ndc = vec2f(
        target_local.x / uniforms.target_size.x * 2.0 - 1.0,
        1.0 - target_local.y / uniforms.target_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.canvas_pos = canvas_pos;
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Mask sampled in its own plane-anchored space (see shaders/lib/canvas.wgsl).
    let mask = sample_mask_plane(
        t_selection,
        t_sampler,
        in.canvas_pos,
        uniforms.mask_offset,
        uniforms.mask_size,
    );
    return vec4f(0.0, 0.0, 0.0, mask);
}
