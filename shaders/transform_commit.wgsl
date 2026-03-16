// Transform-commit: sample a source texture through an inverse affine matrix
// and output directly to a layer/mask texture. Used by commit_floating() to
// write transformed pixels without CPU rasterization.
//
// Unlike the preview shader (transform.wgsl) which reads a background
// accumulator and manually composites, this shader outputs straight-alpha
// pixels and lets the hardware blend state handle compositing.

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

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

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
    // 0.0 = RGBA mode, 1.0 = mask mode (luminance conversion)
    is_mask: f32,
}
@group(0) @binding(2) var<uniform> u: Uniforms;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Convert UV to canvas pixel position
    let canvas_pos = in.uv * u.canvas_size;

    // Transform canvas position to source-local coordinates
    let local = canvas_pos - u.source_origin;

    // Apply inverse affine to find source position
    let src_x = u.inv_row0.x * local.x + u.inv_row0.y * local.y + u.inv_row0.z;
    let src_y = u.inv_row1.x * local.x + u.inv_row1.y * local.y + u.inv_row1.z;

    // Normalize to UV space
    let src_uv = vec2f(src_x, src_y) / u.source_size;

    // Outside source bounds — discard (let the target retain its pixels)
    if (any(src_uv < vec2f(0.0)) || any(src_uv >= vec2f(1.0))) {
        discard;
    }

    // Source texture is stored premultiplied so hardware bilinear
    // interpolation doesn't produce dark halos at content edges.
    let fg_pm = textureSampleLevel(t_source, t_sampler, src_uv, 0.0);
    let a = fg_pm.a * u.opacity;

    if (a <= 0.0) {
        discard;
    }

    // Un-premultiply to get straight-alpha RGB
    let rgb = fg_pm.rgb / fg_pm.a;

    if (u.is_mask > 0.5) {
        // Mask mode: convert RGB to luminance, output as single-channel value.
        // For R8 targets, only the R channel is written. The alpha channel
        // controls source-over blend strength.
        let lum = dot(rgb, vec3f(0.2126, 0.7152, 0.0722));
        return vec4f(lum, lum, lum, a);
    } else {
        // RGBA mode: output straight-alpha color.
        // Hardware blend state does source-over compositing.
        return vec4f(rgb, a);
    }
}
