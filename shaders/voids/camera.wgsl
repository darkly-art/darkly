// Camera void — sample an external image (webcam frame) with a user transform.
//
// Bind group 0:
//   0: Params uniform (inverse user-transform affine, webcam/canvas dims, mirror)
//   1: Source texture (the webcam frame, uploaded by upload_external_image)
//   2: Sampler (linear clamp-to-edge)
//
// MUST stay in lockstep with the CPU mirror `Camera::src_uv` in camera.rs —
// the tests pin them together. Coordinate flow per fragment (all window-local
// pixels; the gizmo edits the affine in the void's local frame, which IS
// window-local, so `canvas_origin` cancels and never appears here):
//   FragCoord.xy → inverse user affine → pre-transform local pixel
//                → cover-fit about canvas center → source-pixel offset
//                → mirror in the source's natural frame
//                → normalize by webcam size → src_uv ∈ [0, 1]
//   src_uv outside [0, 1] → transparent.
//
// `cover = max(canvas_w/src_w, canvas_h/src_h)` makes the webcam exactly cover
// the canvas at the identity transform; the user affine scales/rotates/pans on
// top of that.

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
    webcam_w: f32,
    webcam_h: f32,
    canvas_w: f32,
    canvas_h: f32,
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
    let src_size = vec2f(max(params.webcam_w, 1.0), max(params.webcam_h, 1.0));
    let canvas = vec2f(params.canvas_w, params.canvas_h);

    // Inverse user affine on the window-local fragment → pre-transform local.
    let frag = in.position.xy;
    let local = vec2f(
        params.inv_row0.x * frag.x + params.inv_row0.y * frag.y + params.inv_row0.z,
        params.inv_row1.x * frag.x + params.inv_row1.y * frag.y + params.inv_row1.z,
    );

    // Cover fit about the canvas center.
    let centered = local - canvas * 0.5;
    let cover = max(canvas.x / src_size.x, canvas.y / src_size.y);
    var src_offset = centered / cover;

    // Mirror in the source's natural frame (after the inverse transform), so
    // flipping always flips along the camera's own axis, not the rotated one.
    let mirror = vec2f(1.0 - 2.0 * params.mirror_h, 1.0 - 2.0 * params.mirror_v);
    src_offset = src_offset * mirror;

    let src_uv = src_offset / src_size + vec2f(0.5);

    // textureSample must be called from uniform control flow — sample
    // unconditionally and mask out-of-frame after the fact.
    let sample = textureSample(src_tex, src_sampler, src_uv);
    let in_range =
        src_uv.x >= 0.0 && src_uv.x <= 1.0 &&
        src_uv.y >= 0.0 && src_uv.y <= 1.0;
    return select(vec4f(0.0), sample, in_range);
}
