// The one pass that lands an in-place transform back into the accumulator it
// came from.
//
// Two things composite in place rather than blending a texture in: an effect
// layer, which transforms everything below it, and a masked passthrough group,
// whose children write straight into the parent accumulator. Both produce a
// "before" and an "after" image of the same region, and both then have to
// decide how much of the after to keep:
//
//     Cs     = blend(after, before, mode)
//     result = mix(before, vec4f(Cs, after.a), opacity * mask_alpha)
//
// At opacity 1, mask 1 and mode Normal this reduces to `after` exactly, so the
// unmodulated case is byte-for-byte a passthrough.
//
// Deliberately *not* Porter-Duff source-over. Source-over of a transform's
// result over its own input inflates alpha wherever the input is partially
// transparent (`a + a(1-a) != a`), which is why Krita's adjustment layers
// default to COMPOSITE_COPY (`KoCompositeOpCopy2.h` — a replace, then a lerp on
// the mask and opacity) and GIMP's to GIMP_LAYER_MODE_REPLACE
// (`gimpoperationreplace.c`). Replace-then-lerp is the adjustment-layer
// semantic, and this is that arithmetic.
//
// Masking lives here rather than inside the transform: where a transform lands
// is a property of the site, not of the transform, which is what makes every
// effect maskable without declaring anything.
//
// Bind group 0: before texture, after texture, sampler, uniforms.
// Bind group 1: mask texture (same layout as composite.wgsl group 1).

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

@group(0) @binding(0) var t_before: texture_2d<f32>;
@group(0) @binding(1) var t_after: texture_2d<f32>;
@group(0) @binding(2) var t_sampler: sampler;

// Canvas-window + mask geometry carried inline (not via a separate canvas bind
// group) so the lerp pipeline keeps its two-bind-group layout. The mask is
// sampled in its OWN plane space (via `sample_mask_window`) so a group mask
// that grows independently of the canvas window samples correctly.
struct ApplyUniforms {
    canvas_origin: vec2f,
    canvas_size: vec2f,
    mask_offset: vec2f,
    mask_size: vec2f,
    isolated: u32,
    /// Blend mode's `gpu_value`, selecting an arm of the spliced switch below.
    blend_mode: u32,
    /// How much of the transformed result to keep, before the mask.
    opacity: f32,
    _pad0: u32,
}
@group(0) @binding(3) var<uniform> uniforms: ApplyUniforms;

// Mask texture — same bind group layout as composite.wgsl group 1.
@group(1) @binding(0) var t_mask: texture_2d<f32>;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let mask_alpha = sample_mask_window(
        t_mask,
        t_sampler,
        in.uv,
        uniforms.canvas_origin,
        uniforms.canvas_size,
        uniforms.mask_offset,
        uniforms.mask_size,
    );

    // Show mask as grayscale (same as composite.wgsl behavior).
    if (uniforms.isolated != 0u) {
        return vec4f(mask_alpha, mask_alpha, mask_alpha, 1.0);
    }

    let before = textureSample(t_before, t_sampler, in.uv);
    let after = textureSample(t_after, t_sampler, in.uv);

    // `blend_rgb` takes (source, backdrop) exactly as composite.wgsl does: the
    // transformed image is the source, the image it transformed is the
    // backdrop.
    let Cs = blend_rgb(after, before, uniforms.blend_mode);
    return mix(before, vec4f(Cs, after.a), uniforms.opacity * mask_alpha);
}
