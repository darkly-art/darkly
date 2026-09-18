// Compositing: positions a textured quad on the canvas and composites the
// foreground onto the background with Porter-Duff source-over in the shader.
//
// Used in two contexts:
//   1. Per-dab:        fg = dab texture (premultiplied), bg = canvas copy (straight)
//   2. Stroke→layer:   fg = stroke buffer (straight),    bg = pre-stroke (straight)
// The fg_premultiplied uniform tells the shader which convention fg uses.
//
// Outputs straight alpha with REPLACE blend (no hardware alpha blending).
// See docs/lessons-learned/compositing-lessons-learned.md #4 (why REPLACE) and #6 (why the flag).

struct CompositeUniforms {
    origin: vec2f,       // quad top-left in canvas pixels
    size: vec2f,         // quad size in canvas pixels (= dab diameter)
    target_offset: vec2f, // canvas-space offset of render target's (0,0) pixel
    target_size: vec2f,   // render target pixel dimensions (vertex NDC)
    canvas_size: vec2f,   // document canvas dimensions (fragment selection UV)
    canvas_origin: vec2f, // plane offset of the canvas window (selection-mask anchor)
    uv_min: vec2f,       // min UV in dab texture (nonzero when clipped at top/left)
    uv_max: vec2f,       // max UV in dab texture
    blend_mode: u32,     // 0 = source-over, 1 = erase (destination-out)
    fg_premultiplied: u32, // 1 = dab is premultiplied, 0 = straight alpha
    stroke_opacity: f32, // per-stroke opacity cap (1.0 = no cap). Scales fg alpha before blend.
    apply_selection: u32, // 1 = modulate fg by selection, 0 = ignore selection
    coverage_ceiling: u32, // 1 = deposit only what the pixel can still take
    layering: f32,       // 0 = full ceiling, 1 = plain source-over
}

@group(0) @binding(0) var<uniform> u: CompositeUniforms;
@group(1) @binding(0) var t_dab: texture_2d<f32>;
@group(1) @binding(1) var s_dab: sampler;
@group(2) @binding(0) var t_selection: texture_2d<f32>;
@group(2) @binding(1) var s_selection: sampler;
@group(3) @binding(0) var t_scratch_mirror: texture_2d<f32>;
@group(3) @binding(1) var s_scratch_mirror: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) dab_uv: vec2f,
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
    out.dab_uv = u.uv_min + unit * (u.uv_max - u.uv_min);
    out.canvas_pos = canvas_pos;
    return out;
}

/// Max-norm distance. See the coverage-ceiling block in `fs_main` for why the
/// norm choice matters there.
fn chebyshev(a: vec3f, b: vec3f) -> f32 {
    let v = abs(a - b);
    return max(v.x, max(v.y, v.z));
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Sample foreground (premultiplied or straight, see fg_premultiplied).
    let dab = textureSample(t_dab, s_dab, in.dab_uv);

    // Selection masking: modulate dab by selection coverage. Applied per-dab
    // only: the stroke→layer commit passes `apply_selection = 0` because
    // selection has already been baked into the scratch by prior dabs.
    let sel_uv = plane_to_selection_uv(in.canvas_pos, u.canvas_origin, u.canvas_size);
    let sel_raw = textureSample(t_selection, s_selection, sel_uv).r;
    let sel = select(1.0, sel_raw, u.apply_selection == 1u);

    // Stroke-level opacity cap: scales the foreground alpha (and premultiplied
    // rgb) before the Porter-Duff blend. Per-dab compositing passes 1.0.
    let fg_a = dab.a * sel * u.stroke_opacity;
    // When fg_premultiplied == 0, the dab is straight alpha; premultiply now.
    let fg_rgb_pre = select(dab.rgb * dab.a, dab.rgb, u.fg_premultiplied == 1u) * sel * u.stroke_opacity;

    // Background: read canvas copy (straight alpha).
    // The copy_texture_to_texture origin is floor(u.origin), integer pixel coords.
    // Use floored origin so the UV maps each fragment to the correct canvas texel.
    let copy_uv = (in.canvas_pos - floor(u.origin)) / vec2f(textureDimensions(t_scratch_mirror));
    let bg = textureSample(t_scratch_mirror, s_scratch_mirror, copy_uv);

    // Composite: source-over (paint) or destination-out (erase).
    if u.blend_mode == 1u {
        return destination_out(fg_a, bg);
    }
    if u.coverage_ceiling == 0u {
        return source_over(fg_rgb_pre, fg_a, bg);
    }

    // Coverage ceiling: deposit only what the pixel can still take.
    //
    // A pass carrying pigment `C` at coverage `s` lands, starting from blank,
    // a fixed fraction of the way to `C`. Everything past that is refused. So
    // instead of asking what this pass would add, ask how much room is left
    // between where the pixel already sits and where this pass saturates, and
    // deposit exactly that. A pixel already at the saturation level takes
    // nothing; one that has never been touched takes the full `s`; a heavier
    // pass moves the saturation level and reopens room. No history is read:
    // the room is a property of the pixel's current colour, so a transparent
    // layer and an opaque one holding the same visible mark answer alike.
    //
    // `O` is the origin of the deposit scale, the gamut corner opposite `C`,
    // which is what the distances are measured against. Deriving it from the
    // pigment rather than assuming white is what lets a white pencil on black
    // ground behave exactly like a black one on white.
    if fg_a <= 0.0 {
        return bg;
    }
    let pigment = fg_rgb_pre / fg_a;
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
    let ceiling_t = max(0.0, 1.0 - (1.0 - fg_a) * reach / d);
    // `layering` relaxes the refusal: how readily fresh pigment sits on top of
    // pigment already there, which is the difference between a hard grade that
    // burnishes and a soft one that keeps building. `ceiling_t <= fg_a` always
    // holds (see the max-norm note), so this only ever interpolates between
    // depositing less and depositing exactly what the pass carries.
    let t = mix(ceiling_t, fg_a, clamp(u.layering, 0.0, 1.0));
    return source_over(pigment * t, t, bg);
}
