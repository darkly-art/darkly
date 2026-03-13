// Transform-blend: sample a source texture through an inverse affine matrix,
// then Normal-blend onto the background accumulator. Used by the floating
// content system for both paste-in-place preview and interactive transforms.

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

@group(0) @binding(0) var t_bg: texture_2d<f32>;
@group(0) @binding(1) var t_source: texture_2d<f32>;
@group(0) @binding(2) var t_sampler: sampler;

struct Uniforms {
    // Inverse affine matrix rows: [a, b, tx, _pad] and [c, d, ty, _pad]
    inv_row0: vec4f,
    inv_row1: vec4f,
    // Source texture origin in canvas pixel coords
    source_origin: vec2f,
    // Source texture dimensions in pixels
    source_size: vec2f,
    // Full canvas dimensions in pixels
    canvas_size: vec2f,
    opacity: f32,
    _pad: f32,
}
@group(0) @binding(3) var<uniform> u: Uniforms;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let bg = textureSample(t_bg, t_sampler, in.uv);

    // Convert UV to canvas pixel position
    let canvas_pos = in.uv * u.canvas_size;

    // Transform canvas position to source-local coordinates
    let local = canvas_pos - u.source_origin;

    // Apply inverse affine to find source position
    let src_x = u.inv_row0.x * local.x + u.inv_row0.y * local.y + u.inv_row0.z;
    let src_y = u.inv_row1.x * local.x + u.inv_row1.y * local.y + u.inv_row1.z;

    // Normalize to UV space
    let src_uv = vec2f(src_x, src_y) / u.source_size;

    // Outside source bounds — pass through background
    if (any(src_uv < vec2f(0.0)) || any(src_uv >= vec2f(1.0))) {
        return bg;
    }

    let fg = textureSampleLevel(t_source, t_sampler, src_uv, 0.0);
    let a = fg.a * u.opacity;

    if (a <= 0.0) {
        return bg;
    }

    // Normal blend — straight alpha Porter-Duff source-over
    let out_a = a + bg.a * (1.0 - a);
    let out_rgb = (fg.rgb * a + bg.rgb * bg.a * (1.0 - a)) / max(out_a, 0.001);
    return vec4f(out_rgb, out_a);
}
