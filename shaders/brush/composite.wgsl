// Compositing: positions a textured quad on the canvas and composites the
// foreground onto the background with Porter-Duff source-over in the shader.
//
// Used in two contexts:
//   1. Per-dab:        fg = dab texture (premultiplied), bg = canvas copy (straight)
//   2. Stroke→layer:   fg = stroke buffer (straight),    bg = pre-stroke (straight)
// The fg_premultiplied uniform tells the shader which convention fg uses.
//
// Outputs straight alpha with REPLACE blend — no hardware alpha blending.
// See compositing-lessons-learned.md #4 (why REPLACE) and #6 (why the flag).

struct CompositeUniforms {
    origin: vec2f,       // quad top-left in canvas pixels
    size: vec2f,         // quad size in canvas pixels (= dab diameter)
    canvas_size: vec2f,  // canvas dimensions
    uv_min: vec2f,       // min UV in dab texture (nonzero when clipped at top/left)
    uv_max: vec2f,       // max UV in dab texture
    blend_mode: u32,     // 0 = source-over, 1 = erase (destination-out)
    fg_premultiplied: u32, // 1 = dab is premultiplied, 0 = straight alpha
    stroke_opacity: f32, // per-stroke opacity cap (1.0 = no cap). Scales fg alpha before blend.
    apply_selection: u32, // 1 = modulate fg by selection, 0 = ignore selection
}

@group(0) @binding(0) var<uniform> u: CompositeUniforms;
@group(1) @binding(0) var t_dab: texture_2d<f32>;
@group(1) @binding(1) var s_dab: sampler;
@group(2) @binding(0) var t_selection: texture_2d<f32>;
@group(2) @binding(1) var s_selection: sampler;
@group(3) @binding(0) var t_canvas_copy: texture_2d<f32>;
@group(3) @binding(1) var s_canvas_copy: sampler;

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

    // Canvas pixels -> NDC, Y flipped.
    let ndc = vec2f(
        canvas_pos.x / u.canvas_size.x * 2.0 - 1.0,
        1.0 - canvas_pos.y / u.canvas_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.dab_uv = u.uv_min + unit * (u.uv_max - u.uv_min);
    out.canvas_pos = canvas_pos;
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Sample foreground (premultiplied or straight — see fg_premultiplied).
    let dab = textureSample(t_dab, s_dab, in.dab_uv);

    // Selection masking: modulate dab by selection coverage. Applied per-dab
    // only — the stroke→layer commit passes `apply_selection = 0` because
    // selection has already been baked into the scratch by prior dabs.
    let sel_uv = in.canvas_pos / u.canvas_size;
    let sel_raw = textureSample(t_selection, s_selection, sel_uv).r;
    let sel = select(1.0, sel_raw, u.apply_selection == 1u);

    // Stroke-level opacity cap: scales the foreground alpha (and premultiplied
    // rgb) before the Porter-Duff blend. Per-dab compositing passes 1.0.
    let fg_a = dab.a * sel * u.stroke_opacity;
    // When fg_premultiplied == 0, the dab is straight alpha — premultiply now.
    let fg_rgb_pre = select(dab.rgb * dab.a, dab.rgb, u.fg_premultiplied == 1u) * sel * u.stroke_opacity;

    // Background: read canvas copy (straight alpha).
    // The copy_texture_to_texture origin is floor(u.origin) — integer pixel coords.
    // Use floored origin so the UV maps each fragment to the correct canvas texel.
    let copy_uv = (in.canvas_pos - floor(u.origin)) / vec2f(textureDimensions(t_canvas_copy));
    let bg = textureSample(t_canvas_copy, s_canvas_copy, copy_uv);

    // Composite: source-over (paint) or destination-out (erase).
    if u.blend_mode == 1u {
        return destination_out(fg_a, bg);
    }
    return source_over(fg_rgb_pre, fg_a, bg);
}
