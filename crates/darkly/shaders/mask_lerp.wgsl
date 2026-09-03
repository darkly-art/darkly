// Mask lerp shader for passthrough groups with masks (Photoshop behavior).
//
// A passthrough group composites its children directly into the parent
// accumulator.  When such a group also has a mask, we snapshot the parent
// accumulator *before* compositing the children, then lerp between the
// snapshot (before) and the result (after) using the mask:
//
//     result = mix(before, after, mask_alpha)
//
// This preserves passthrough semantics (each child's blend mode interacts
// with the parent's content) while the mask controls how much of the
// group's contribution is visible.
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
struct LerpUniforms {
    canvas_origin: vec2f,
    canvas_size: vec2f,
    mask_offset: vec2f,
    mask_size: vec2f,
    isolated: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}
@group(0) @binding(3) var<uniform> uniforms: LerpUniforms;

// Mask texture: same bind group layout as composite.wgsl group 1.
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
    return mix(before, after, mask_alpha);
}
