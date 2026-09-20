// The stroke commit: lays a stroke's finished accumulations onto the layer.
//
// Two foreground slots, each with a fixed law and its own opacity, where an
// opacity of zero means the slot is absent:
//   wash:  committed through the per-pigment deposit ceiling
//   build: composited on top with plain Porter-Duff source-over
//
// A terminal maps its accumulations onto the slots; the shader knows nothing
// about brushes. One accumulation under one law is the common case (a brush
// at either end of `paint.buildup`, or watercolor), and a brush inside the
// dial fills both. Both foregrounds are premultiplied, which is how every
// terminal's scratch and channels accumulate; the background (the pre-stroke
// snapshot) is straight alpha, and so is the output.
//
// Outputs straight alpha with REPLACE blend (no hardware alpha blending).
// See docs/lessons-learned/compositing-lessons-learned.md #4 (why REPLACE).

struct CompositeUniforms {
    origin: vec2f,       // quad top-left in canvas pixels
    size: vec2f,         // quad size in canvas pixels
    target_offset: vec2f, // canvas-space offset of render target's (0,0) pixel
    target_size: vec2f,   // render target pixel dimensions (vertex NDC)
    uv_min: vec2f,       // min UV in the foreground textures
    uv_max: vec2f,       // max UV in the foreground textures
    blend_mode: u32,     // 0 = source-over, 1 = erase (destination-out)
    wash_opacity: f32,   // stroke opacity of the wash slot; 0 = absent
    build_opacity: f32,  // stroke opacity of the build slot; 0 = absent
}

@group(0) @binding(0) var<uniform> u: CompositeUniforms;
@group(1) @binding(0) var t_wash: texture_2d<f32>;
@group(1) @binding(1) var s_wash: sampler;
@group(2) @binding(0) var t_build: texture_2d<f32>;
@group(2) @binding(1) var s_build: sampler;
@group(3) @binding(0) var t_bg: texture_2d<f32>;
@group(3) @binding(1) var s_bg: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) fg_uv: vec2f,
    @location(1) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Quad from 6 vertices (two triangles): 0,1,2, 2,1,3
    //   0──1      unit corners: (0,0) (1,0) (0,1) (1,1)
    //   │╲ │      tri 0: 0,1,2  tri 1: 2,1,3
    //   2──3
    let corner = array<vec2f, 6>(
        vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
        vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    );
    let unit = corner[idx];
    let canvas_pos = u.origin + unit * u.size;

    // Translate canvas-space → target-local, then to NDC against target size.
    let target_local = canvas_pos - u.target_offset;
    let ndc = vec2f(
        target_local.x / u.target_size.x * 2.0 - 1.0,
        1.0 - target_local.y / u.target_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.fg_uv = u.uv_min + unit * (u.uv_max - u.uv_min);
    out.canvas_pos = canvas_pos;
    return out;
}

/// Max-norm distance. See `deposit_through_ceiling` for why the norm choice
/// matters there.
fn chebyshev(a: vec3f, b: vec3f) -> f32 {
    let v = abs(a - b);
    return max(v.x, max(v.y, v.z));
}

// The deposit ceiling: lay `fg` (premultiplied) onto `bg`, depositing only
// what the pixel can still take.
//
// A pass carrying pigment `C` at coverage `s` lands, starting from blank, a
// fixed fraction of the way to `C`. Everything past that is refused. So
// instead of asking what this pass would add, ask how much room is left
// between where the pixel already sits and where this pass saturates, and
// deposit exactly that. A pixel already at the saturation level takes
// nothing; one that has never been touched takes the full `s`; a heavier
// pass moves the saturation level and reopens room. No history is read: the
// room is a property of the pixel's current colour, so a transparent layer
// and an opaque one holding the same visible mark answer alike.
//
// `O` is the origin of the deposit scale, the gamut corner opposite `C`,
// which is what the distances are measured against. Deriving it from the
// pigment rather than assuming white is what lets a white pencil on black
// ground behave exactly like a black one on white.
fn deposit_through_ceiling(fg: vec4f, bg: vec4f) -> vec4f {
    if fg.a <= 0.0 {
        return bg;
    }
    let pigment = fg.rgb / fg.a;
    let origin = select(vec3f(0.0), vec3f(1.0), pigment < vec3f(0.5));

    // The max-norm is load-bearing, not a cheap stand-in for a Euclidean one.
    // Under it, `d <= reach` holds for every colour in the cube, so a pass can
    // only ever be reduced, never amplified, and `t` collapses to exactly `s`
    // on any untouched ground. Under a Euclidean norm that is false: white is
    // not red's antipode, so a red pencil on white paper would saturate at a
    // weaker mark than graphite does at the same pressure.
    //
    // This is also where a move to OKLab would land. Distance there is
    // Euclidean and perceptually uniform, which is the property this actually
    // wants, but it needs a different reference than the cube corner to keep
    // the `d <= reach` guarantee. Worth revisiting with the colour-system
    // rewrite, not before.
    let reach = chebyshev(origin, pigment);
    let ground = bg.rgb * bg.a + origin * (1.0 - bg.a);
    let d = chebyshev(ground, pigment);
    if d <= 0.0 {
        return bg;
    }
    // `t <= fg.a` always holds (see the max-norm note), so the ceiling can
    // only ever reduce what the pass carries, never amplify it.
    let t = max(0.0, 1.0 - (1.0 - fg.a) * reach / d);
    return source_over(pigment * t, t, bg);
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Background: the pre-stroke snapshot, straight alpha. The
    // copy_texture_to_texture origin is floor(u.origin), integer pixel
    // coords, so the floored origin maps each fragment to its own texel.
    let copy_uv = (in.canvas_pos - floor(u.origin)) / vec2f(textureDimensions(t_bg));
    let bg = textureSample(t_bg, s_bg, copy_uv);

    // Each slot scaled by its own stroke opacity. Premultiplied, so one
    // multiply covers rgb and alpha together.
    let wash = textureSample(t_wash, s_wash, in.fg_uv) * u.wash_opacity;
    let build = textureSample(t_build, s_build, in.fg_uv) * u.build_opacity;

    if u.blend_mode == 1u {
        // Erase: each slot removes its own coverage, composing to a removal
        // of `1 - (1 - wash.a) * (1 - build.a)`. Removal never goes through
        // the ceiling, so an eraser can always reach zero. No gate needed:
        // `destination_out(0, x)` is exactly `x`.
        return destination_out(build.a, destination_out(wash.a, bg));
    }

    // The wash slot first: the ceiling reads the ground to find room, and a
    // build slot laid under it would let a stroke's own build-up shrink its
    // own wash. An absent slot is skipped rather than composited at zero
    // alpha, because `source_over` at zero alpha is not an exact identity
    // (it divides `bg.a * bg.rgb` by `bg.a`, and zeroes rgb under 0.001).
    var out = bg;
    if u.wash_opacity > 0.0 {
        out = deposit_through_ceiling(wash, out);
    }
    if u.build_opacity > 0.0 {
        out = source_over(build.rgb, build.a, out);
    }
    return out;
}
