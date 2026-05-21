// Camera void — sample an external image (webcam frame) with user transforms.
//
// Bind group 0:
//   0: Params uniform (scale, rotation_rad, pan, webcam/canvas dims)
//   1: Source texture (the webcam frame, uploaded by upload_external_image)
//   2: Sampler (linear clamp-to-edge)
//
// Coordinate flow per fragment, all in *pixel* space so rotation preserves
// shape independent of canvas aspect:
//   FragCoord.xy → dest_centered (pixels, origin at canvas center)
//                → inverse-rotate (image rotates, not the UV grid)
//                → divide by total_scale = cover_scale * user_scale
//                → subtract pan (canvas-fraction units → source pixels)
//                → normalize by webcam size → src_uv ∈ [0, 1]
//   src_uv outside [0, 1] → transparent.
//
// `cover_scale = max(canvas_w/src_w, canvas_h/src_h)` is the factor that
// makes the webcam exactly cover the canvas at scale=1. Doing the user's
// scale on top of cover_scale gives the natural "1.0 = cover-fit, >1 zooms
// in, <1 reveals letterbox" feel.

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
    scale: f32,
    rotation_rad: f32,
    pan_x: f32,
    pan_y: f32,
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
    let canvas = vec2f(params.canvas_w, params.canvas_h);
    let src_size = vec2f(max(params.webcam_w, 1.0), max(params.webcam_h, 1.0));

    // Centered destination pixel (pixels, origin at canvas center).
    let dest_centered = in.position.xy - canvas * 0.5;

    // Inverse rotation in pixel space — preserves shape regardless of
    // canvas aspect, unlike rotating in normalized [0,1] UV which skews.
    let c = cos(-params.rotation_rad);
    let s = sin(-params.rotation_rad);
    let dest_rot = vec2f(
        c * dest_centered.x - s * dest_centered.y,
        s * dest_centered.x + c * dest_centered.y,
    );

    // Cover fit factor + user zoom. At user_scale = 1 the source's short
    // axis exactly fills the canvas's matching axis; the long axis crops.
    let cover_scale = max(canvas.x / src_size.x, canvas.y / src_size.y);
    let total_scale = cover_scale * max(params.scale, 1e-6);

    // Inverse scale → source-pixel offset from source center.
    var src_offset = dest_rot / total_scale;

    // Mirror happens *after* rotation but before pan, so flipping
    // horizontally always flips along the source's natural x axis
    // (not along the rotated dest x axis). This matches how every
    // selfie camera in the world behaves: rotate the head, the
    // mirroring stays bound to the camera's frame, not your head.
    let mirror = vec2f(1.0 - 2.0 * params.mirror_h, 1.0 - 2.0 * params.mirror_v);
    src_offset = src_offset * mirror;

    // Pan in canvas-fraction units (1.0 = full canvas dim). Converted to
    // source pixels via the same total_scale so a pan of 1 shifts the
    // image by exactly one canvas-worth, independent of zoom or aspect.
    let pan_src = vec2f(params.pan_x, params.pan_y) * canvas / total_scale;

    let src_uv = (src_offset - pan_src) / src_size + vec2f(0.5);

    // textureSample must be called from uniform control flow — sample
    // unconditionally and mask out-of-frame after the fact.
    let sample = textureSample(src_tex, src_sampler, src_uv);
    let in_range =
        src_uv.x >= 0.0 && src_uv.x <= 1.0 &&
        src_uv.y >= 0.0 && src_uv.y <= 1.0;
    return select(vec4f(0.0), sample, in_range);
}
