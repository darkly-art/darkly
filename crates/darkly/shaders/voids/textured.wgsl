// Video-stream void: sample an external image (webcam / screenshare frame)
// with an artist transform.
//
// Bind group 0:
//   0: Params uniform (inverse artist-transform affine, cover-fit content rect)
//   1: Source texture (the video frame, uploaded by upload_external_image)
//   2: Sampler (linear clamp-to-edge)
//
// Alpha convention: the source texture stores PREMULTIPLIED texels so the
// linear filter interpolates correctly at alpha edges; filtering straight
// alpha darkens color toward transparent-black neighbors (dark halos;
// docs/lessons-learned/compositing-lessons-learned.md #2). The fragment
// un-premultiplies after sampling, returning the straight alpha the
// compositor expects. 8-bit premultiplied storage quantizes color at very
// low alpha (inherent to the approach, imperceptible next to halos).
//
// MUST stay in lockstep with the CPU mirror `TexturedVoid::src_uv` in
// textured_void.rs; the tests pin them together. The inverse-homography
// sample is the shared `proj_local` (lib/projective.wgsl), concatenated ahead
// of this file at pipeline build. Coordinate flow per fragment (all
// window-local pixels; the gizmo edits the transform in the content rect's
// local frame, so `canvas_origin` cancels and never appears here):
//   FragCoord.xy → subtract content_origin → content-local pixel
//                → inverse artist homography (perspective divide) → pre-transform
//                  content-local pixel
//                → normalize by content_size → src_uv ∈ [0, 1]
//   src_uv outside [0, 1] (or a degenerate, behind-camera sample) → transparent.
//
// The silhouette is antialiased analytically: alpha is scaled by the fraction of
// the fragment's footprint in source space that falls inside [0, 1]², measured
// from the screen-space derivatives of `src_uv`. That is a one-destination-pixel
// edge at every scale, and it is independent of which mip level the hardware
// picked. The alternative, ringing the source with transparent texels and
// letting the filter fade out against them, breaks under minification, because
// each reduction halves the ring's width relative to the level it lives in
// until clamp-to-edge is projecting a nearly opaque edge across the canvas.
//
// Cover-fit is baked into the content rect (origin + size), computed CPU-side
// in `TexturedVoid::content_rect`; at the identity transform the source
// exactly fills that rect, which overhangs the canvas on the cropped axis.
// Mirroring is no longer a shader concern: it's expressed as a negative scale
// in the gizmo transform, which the inverse above samples through for free.

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
    // Inverse of the artist transform's homography, packed rows [m, _] (see
    // gpu::transform::pack_inv_rows). Affine carries inv_row2 = [0,0,1,_].
    inv_row0: vec4f,
    inv_row1: vec4f,
    inv_row2: vec4f,
    // Content rect in window-local coords (a Cover fit overhangs the canvas on
    // the cropped axis). The fit is baked in CPU-side.
    content_origin: vec2f,
    content_size: vec2f,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Window-local fragment → content-local → inverse artist homography (shared
    // proj_local; .z flags a degenerate / behind-camera sample).
    let cl = in.position.xy - params.content_origin;
    let pre = proj_local(params.inv_row0, params.inv_row1, params.inv_row2, cl);

    let src_uv = pre.xy / params.content_size;
    let sample = textureSample(src_tex, src_sampler, src_uv);

    // Coverage of the image rect over this fragment's footprint, per axis: the
    // overlap between the footprint interval and [0, 1], as a fraction of the
    // footprint. 1 well inside, 0 well outside, and the exact partial area in
    // between, which is what antialiases the silhouette of a rotated or scaled
    // layer. A boolean in/out test would round that partial coverage to 0 or 1
    // and stair-step every non-axis-aligned edge; leaving the sampler to fade
    // out against padding would fail under minification (see the header).
    let half_fp = max(fwidth(src_uv) * 0.5, vec2f(1e-6));
    let overlap = min(src_uv + half_fp, vec2f(1.0)) - max(src_uv - half_fp, vec2f(0.0));
    let cov = clamp(overlap / (2.0 * half_fp), vec2f(0.0), vec2f(1.0));

    // Un-premultiply the filtered sample back to straight alpha. The epsilon
    // sends α=0 texels to rgb 0; rgb ≤ α holds for filtered premultiplied
    // texels, so no clamp is needed.
    let straight = vec4f(sample.rgb / max(sample.a, 1e-4), sample.a * cov.x * cov.y);
    // `pre.z` is the projective-validity flag, not a boundary test: it marks a
    // degenerate or behind-camera sample, which has no meaningful UV at all.
    return select(vec4f(0.0), straight, pre.z > 0.5);
}
