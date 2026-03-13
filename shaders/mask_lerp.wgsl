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

struct LerpUniforms {
    show_mask: u32,
}
@group(0) @binding(3) var<uniform> uniforms: LerpUniforms;

// Mask texture — same bind group layout as composite.wgsl group 1.
@group(1) @binding(0) var t_mask: texture_2d<f32>;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let mask_alpha = textureSample(t_mask, t_sampler, in.uv).r;

    // Show mask as grayscale (same as composite.wgsl behavior).
    if (uniforms.show_mask != 0u) {
        return vec4f(mask_alpha, mask_alpha, mask_alpha, 1.0);
    }

    let before = textureSample(t_before, t_sampler, in.uv);
    let after = textureSample(t_after, t_sampler, in.uv);
    return mix(before, after, mask_alpha);
}
