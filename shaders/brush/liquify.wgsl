// Liquify warp: sample the stroke scratch (via scratch read mirror) at a displaced
// UV inside a circular brush disc and write the result back to the scratch.
// Each dab pushes pixels by `-direction × displacement × falloff`. Pen
// speed is deliberately absent — `displacement` is CPU-computed from
// `radius × K × strength` alone, so slow and fast drags feel the same per
// dab. Speed only shows up indirectly via how many dabs fire per unit time.
//
// Repeated dabs along a stroke compound because the shader reads the
// cumulatively-warped scratch, not the pre-stroke layer.
//
// Discard outside the radius so existing scratch content is preserved
// (LoadOp::Load on the render pass). REPLACE blend — fragment output is the
// final pixel value.
//
// Softness waveshape (Krita's warp brush uses Gaussian; we parameterise):
//   softness = 0   → spike   (pow(1-d, 8): sharp peak, almost zero past mid-radius)
//   softness = 0.4 → saw     (linear 1-d)
//   softness = 0.5 → sine    (cosine bump, zero-slope at endpoints)
//   softness = 1   → square  (hard-edged disc, step function)
// Spike→saw fills [0, 0.4] via the pow exponent ramping 8→1; saw→sine
// is a narrow blend [0.4, 0.5] (the difference is visually subtle so it
// doesn't deserve much room); sine→square fills [0.5, 1].

struct LiquifyUniforms {
    rect_origin:   vec2f,  // quad top-left in canvas pixels (clamped to layer extent)
    rect_size:     vec2f,  // quad w,h in canvas pixels
    target_offset: vec2f,  // layer's canvas-space (offset_x, offset_y)
    target_size:   vec2f,  // layer pixel dimensions (vertex NDC denom)
    canvas_size:   vec2f,  // full canvas (selection UV only)
    copy_origin:   vec2f,  // layer-local origin of scratch read mirror region (float form)
    center:        vec2f,  // brush centre in canvas pixels
    direction:     vec2f,  // unit vector of pen travel
    displacement:  f32,    // canvas-pixel shift at falloff = 1
    radius:        f32,
    softness:      f32,
    _pad:          f32,
}

@group(0) @binding(0) var<uniform> u: LiquifyUniforms;
@group(1) @binding(0) var t_selection: texture_2d<f32>;
@group(1) @binding(1) var s_selection: sampler;
@group(2) @binding(0) var t_scratch_mirror: texture_2d<f32>;
@group(2) @binding(1) var s_scratch_mirror: sampler;

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

    // The render target is the layer-sized stroke scratch (offset by
    // u.target_offset in canvas space). Map canvas pixels through the
    // layer's local frame so paste-extent / grown layers land in the
    // correct scratch texel. Y flipped.
    let target_pos = canvas_pos - u.target_offset;
    let ndc = vec2f(
        target_pos.x / u.target_size.x * 2.0 - 1.0,
        1.0 - target_pos.y / u.target_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.canvas_pos = canvas_pos;
    return out;
}

/// Falloff: waveshape morph driven by softness ∈ [0, 1].
fn falloff(d: f32, softness: f32) -> f32 {
    // Each piece is 1 at d=0 and 0 at d=1, monotonically decreasing.
    let saw  = 1.0 - d;
    let sine = 0.5 + 0.5 * cos(3.14159265 * d);
    let saw_break  = 0.4;        // softness at which the spike has fully relaxed to saw
    let sine_break = 0.5;        // softness at which saw has fully blended to sine
    let k_max      = 8.0;        // sharpest spike exponent (0.5^8 ≈ 0.004 at mid-radius)
    if softness <= saw_break {
        let t = softness / saw_break;              // 0 → spike, 1 → saw
        let k = mix(k_max, 1.0, t);
        return pow(max(saw, 0.0), k);
    } else if softness <= sine_break {
        let t = (softness - saw_break) / (sine_break - saw_break);
        return mix(saw, sine, t);
    } else {
        let t = (softness - sine_break) / (1.0 - sine_break);
        return mix(sine, 1.0, t);                  // square = 1.0; edge handled by outside-radius discard
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

    // Displace sampling position opposite the drawing direction — pushing
    // pixels along the direction means reading from "behind" where the pen
    // came from. Magnitude is CPU-computed from strength × radius; speed-
    // independent.
    let source_pos = canvas_pos - u.direction * u.displacement * f;

    // UV into scratch read mirror using the same floor(origin) convention as
    // composite.wgsl (gpu-lessons-learned.md §7). Both source_pos and
    // copy_origin are translated into the layer's local frame
    // (subtract target_offset for source_pos; copy_origin is already
    // layer-local from CPU side) before recovering the texel.
    let copy_uv = (source_pos - u.target_offset - floor(u.copy_origin))
        / vec2f(textureDimensions(t_scratch_mirror));
    let warped = textureSample(t_scratch_mirror, s_scratch_mirror, copy_uv);

    // Selection mask is canvas-sized — UV stays in canvas coords.
    let sel_uv = canvas_pos / u.canvas_size;
    let sel = textureSample(t_selection, s_selection, sel_uv).r;
    // Undisplaced sample (the pixel at canvas_pos before this dab) for
    // selection blending. Same canvas → layer-local translation.
    let original_uv = (canvas_pos - u.target_offset - floor(u.copy_origin))
        / vec2f(textureDimensions(t_scratch_mirror));
    let original = textureSample(t_scratch_mirror, s_scratch_mirror, original_uv);

    return mix(original, warped, sel);
}
