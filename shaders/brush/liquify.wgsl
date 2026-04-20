// Liquify warp: sample the stroke scratch (via canvas_copy) at a displaced
// UV inside a circular brush disc and write the result back to the scratch.
// Each dab pushes pixels by -motion * falloff * strength; repeated dabs
// along a stroke compound because the shader reads the cumulatively-warped
// scratch, not the pre-stroke layer.
//
// Discard outside the radius so existing scratch content is preserved
// (LoadOp::Load on the render pass). REPLACE blend — fragment output is the
// final pixel value.
//
// Softness waveshape (Krita's warp brush uses Gaussian; we parameterise):
//   softness = 0   → saw     (linear 1-d)
//   softness = 0.5 → sine    (cosine bump, zero-slope at endpoints)
//   softness = 1   → square  (hard-edged disc, step function)
// Interpolated linearly between these three shapes.

struct LiquifyUniforms {
    rect_origin: vec2f,  // quad top-left in canvas pixels
    rect_size:   vec2f,  // quad w,h in canvas pixels
    canvas_size: vec2f,  // full canvas
    copy_origin: vec2f,  // float form of the integer copy origin
    center:      vec2f,  // brush centre in canvas pixels
    motion:      vec2f,  // per-dab motion (canvas pixels)
    radius:      f32,
    strength:    f32,
    softness:    f32,
    _pad:        f32,
}

@group(0) @binding(0) var<uniform> u: LiquifyUniforms;
@group(1) @binding(0) var t_selection: texture_2d<f32>;
@group(1) @binding(1) var s_selection: sampler;
@group(2) @binding(0) var t_canvas_copy: texture_2d<f32>;
@group(2) @binding(1) var s_canvas_copy: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Same quad layout as composite.wgsl — six vertices, two triangles.
    let corner = array<vec2f, 6>(
        vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
        vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    );
    let unit = corner[idx];
    let canvas_pos = u.rect_origin + unit * u.rect_size;

    // Canvas pixels → NDC, Y flipped.
    let ndc = vec2f(
        canvas_pos.x / u.canvas_size.x * 2.0 - 1.0,
        1.0 - canvas_pos.y / u.canvas_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.canvas_pos = canvas_pos;
    return out;
}

/// Falloff: waveshape morph driven by softness.
fn falloff(d: f32, softness: f32) -> f32 {
    // Each piece is 1 at d=0 and 0 at d=1, monotonically decreasing.
    let saw    = 1.0 - d;
    let sine   = 0.5 + 0.5 * cos(3.14159265 * d);
    let square = 1.0;   // hard cutoff is handled by the outside-radius discard
    if softness <= 0.5 {
        let t = softness * 2.0;            // 0 → saw, 1 → sine
        return mix(saw, sine, t);
    } else {
        let t = (softness - 0.5) * 2.0;    // 0 → sine, 1 → square
        return mix(sine, square, t);
    }
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let canvas_pos = in.canvas_pos;

    // Normalised radial distance (0 at center, 1 at radius).
    let d = distance(canvas_pos, u.center) / max(u.radius, 1e-5);
    if d >= 1.0 {
        // Outside the disc: leave the scratch untouched.
        discard;
    }

    // Waveshape falloff.
    let f = falloff(d, clamp(u.softness, 0.0, 1.0));

    // Displace sampling position opposite the motion — pushing pixels along
    // motion means reading from "behind" where the motion came from.
    let source_pos = canvas_pos - u.motion * f * u.strength;

    // UV into canvas_copy using the same floor(origin) convention as
    // composite.wgsl (gpu-lessons-learned.md §7).
    let copy_uv = (source_pos - floor(u.copy_origin))
        / vec2f(textureDimensions(t_canvas_copy));
    let warped = textureSample(t_canvas_copy, s_canvas_copy, copy_uv);

    // Selection mask: unselected regions leave the scratch at its prior
    // value (the fragment at canvas_pos before this dab — which lives at the
    // same position in canvas_copy, undisplaced).
    let sel_uv = canvas_pos / u.canvas_size;
    let sel = textureSample(t_selection, s_selection, sel_uv).r;
    let original_uv = (canvas_pos - floor(u.copy_origin))
        / vec2f(textureDimensions(t_canvas_copy));
    let original = textureSample(t_canvas_copy, s_canvas_copy, original_uv);

    return mix(original, warped, sel);
}
