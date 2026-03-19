// Dab texture compositing: positions a textured quad on the canvas and
// composites the dab with Porter-Duff source-over in the shader.
//
// The canvas region under the dab is copied to a separate texture before
// this pass runs.  The shader reads both the dab and canvas copy, computes
// correct straight-alpha source-over, and outputs with REPLACE blend.
// This avoids the premultiplied-stored-as-straight bug that hardware alpha
// blending causes on straight-alpha layer textures.

struct CompositeUniforms {
    origin: vec2f,       // quad top-left in canvas pixels
    size: vec2f,         // quad size in canvas pixels (= dab diameter)
    canvas_size: vec2f,  // canvas dimensions
    uv_min: vec2f,       // min UV in dab texture (nonzero when clipped at top/left)
    uv_max: vec2f,       // max UV in dab texture
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
    // Sample dab (premultiplied alpha — correct for bilinear filtering).
    let dab = textureSample(t_dab, s_dab, in.dab_uv);

    // Selection masking: modulate dab by selection coverage.
    let sel_uv = in.canvas_pos / u.canvas_size;
    let sel = textureSample(t_selection, s_selection, sel_uv).r;
    let fg_a = dab.a * sel;
    let fg_rgb_pre = dab.rgb * sel;

    // Background: read canvas copy (straight alpha).
    // UV maps quad pixel position to the copied region at (0,0) in the copy texture.
    let copy_uv = (in.canvas_pos - u.origin) / vec2f(textureDimensions(t_canvas_copy));
    let bg = textureSample(t_canvas_copy, s_canvas_copy, copy_uv);

    // Porter-Duff source-over (premultiplied fg, straight bg) → straight output.
    let out_a = fg_a + bg.a * (1.0 - fg_a);
    let out_rgb = select(
        vec3f(0.0),
        (fg_rgb_pre + (1.0 - fg_a) * bg.a * bg.rgb) / out_a,
        out_a > 0.001,
    );

    return vec4f(out_rgb, out_a);
}
