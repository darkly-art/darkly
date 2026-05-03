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
    // Canvas-space offset of the render target's (0,0) pixel.
    target_offset: vec2f,
    // Render target pixel dimensions.
    target_size: vec2f,
    // Full document canvas dimensions in pixels.
    canvas_size: vec2f,
    opacity: f32,
    // Used only by the commit shader; the preview shader ignores it.
    is_r8: f32,
    // Target layer offset & size in canvas pixels — used for sampling the
    // target layer's mask. The mask shares the layer's bounds.
    layer_offset: vec2f,
    layer_size: vec2f,
}
@group(0) @binding(3) var<uniform> u: Uniforms;

// Target layer mask — same bind group layout as the blend pass uses.
// When the target has no mask, a 1x1 white fallback is bound.
@group(1) @binding(0) var t_mask: texture_2d<f32>;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let bg = textureSample(t_bg, t_sampler, in.uv);

    // Convert target UV to canvas pixel position via the target's canvas-space
    // origin and size. For canvas-aligned targets (target_offset=0,
    // target_size=canvas_size) this collapses to `uv * canvas_size`.
    let canvas_pos = u.target_offset + in.uv * u.target_size;

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

    // Source texture is stored premultiplied so hardware bilinear
    // interpolation doesn't produce dark halos at content edges.
    let fg_pm = textureSampleLevel(t_source, t_sampler, src_uv, 0.0);

    // Sample the target layer's mask at the canvas position. The mask shares
    // the layer's bounds; outside those bounds the mask reveals nothing
    // (matches composite.wgsl's clamp-to-zero behavior). Apply the mask alpha
    // as a coverage multiplier so the floating preview matches what the
    // committed pixels will look like once the regular blend pass re-applies
    // the same mask at compositing time.
    let layer_uv = (canvas_pos - u.layer_offset) / u.layer_size;
    let in_layer = all(layer_uv >= vec2f(0.0)) && all(layer_uv <= vec2f(1.0));
    // textureSampleLevel (explicit LOD) is required here because we sit
    // after the early-return at the top of the function — implicit-derivative
    // textureSample is illegal in non-uniform control flow.
    let mask_raw = textureSampleLevel(t_mask, t_sampler, layer_uv, 0.0).r;
    let mask_alpha = select(0.0, mask_raw, in_layer);

    let a = fg_pm.a * u.opacity * mask_alpha;

    if (a <= 0.0) {
        return bg;
    }

    // Porter-Duff source-over (premultiplied fg, straight bg → straight output).
    let fg_pre = fg_pm.rgb * u.opacity * mask_alpha;
    return source_over(fg_pre, a, bg);
}
