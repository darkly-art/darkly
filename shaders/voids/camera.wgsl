// Camera void — sample an external image (webcam frame) with a user transform.
//
// Bind group 0:
//   0: Params uniform (inverse user-transform affine, webcam/canvas dims, mirror)
//   1: Source texture (the webcam frame, uploaded by upload_external_image)
//   2: Sampler (linear clamp-to-edge)
//
// MUST stay in lockstep with the CPU mirror `Camera::src_uv` in camera.rs —
// the tests pin them together. Coordinate flow per fragment (all window-local
// pixels; the gizmo edits the affine in the content rect's local frame, so
// `canvas_origin` cancels and never appears here):
//   FragCoord.xy → subtract content_origin → content-local pixel
//                → inverse user affine → pre-transform content-local pixel
//                → normalize by content_size → src_uv ∈ [0, 1]
//                → mirror about the UV center
//   src_uv outside [0, 1] → transparent.
//
// Cover-fit is baked into the content rect (origin + size), computed CPU-side
// in `Camera::content_rect`; at the identity transform the webcam exactly fills
// that rect, which overhangs the canvas on the cropped axis.

struct VertexOutput {
    @builtin(position) position: vec4f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

struct Params {
    // Inverse of the user transform's affine, row-major rows [a, b, tx, _].
    inv_row0: vec4f,
    inv_row1: vec4f,
    // Cover-fit content rect in window-local coords (origin overhangs the
    // canvas on the cropped axis). Cover-fit is baked in CPU-side.
    content_origin: vec2f,
    content_size: vec2f,
    // 0.0 or 1.0. `1.0 - 2.0 * mirror` is the sign multiplier — flips
    // the corresponding axis when on, identity when off.
    mirror_h: f32,
    mirror_v: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Window-local fragment → content-local → inverse user affine.
    let cl = in.position.xy - params.content_origin;
    let pre = vec2f(
        params.inv_row0.x * cl.x + params.inv_row0.y * cl.y + params.inv_row0.z,
        params.inv_row1.x * cl.x + params.inv_row1.y * cl.y + params.inv_row1.z,
    );

    // Normalize to the source UV, then mirror about the UV center so flipping
    // is along the camera's own axis (not the rotated one).
    var src_uv = pre / params.content_size;
    let mirror = vec2f(1.0 - 2.0 * params.mirror_h, 1.0 - 2.0 * params.mirror_v);
    src_uv = vec2f(0.5) + (src_uv - vec2f(0.5)) * mirror;

    // textureSample must be called from uniform control flow — sample
    // unconditionally and mask out-of-frame after the fact.
    let sample = textureSample(src_tex, src_sampler, src_uv);
    let in_range =
        src_uv.x >= 0.0 && src_uv.x <= 1.0 &&
        src_uv.y >= 0.0 && src_uv.y <= 1.0;
    return select(vec4f(0.0), sample, in_range);
}
