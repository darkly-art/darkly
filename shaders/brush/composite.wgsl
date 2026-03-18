// Dab texture compositing: positions a textured quad on the canvas and
// samples the dab texture.  Hardware alpha-over blend handles compositing.
// Selection masking modulates the dab alpha.

struct CompositeUniforms {
    origin: vec2f,       // quad top-left in canvas pixels
    size: vec2f,         // quad size in canvas pixels (= dab diameter)
    canvas_size: vec2f,  // canvas dimensions
    uv_max: vec2f,       // max UV in dab texture (= dab_diameter / tex_size)
}

@group(0) @binding(0) var<uniform> u: CompositeUniforms;
@group(1) @binding(0) var t_dab: texture_2d<f32>;
@group(1) @binding(1) var s_dab: sampler;
@group(2) @binding(0) var t_selection: texture_2d<f32>;
@group(2) @binding(1) var s_selection: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) dab_uv: vec2f,
    @location(1) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle trick mapped to the dab quad region.
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    let canvas_pos = u.origin + unit * u.size;

    // Canvas pixels -> NDC, Y flipped.
    let ndc = vec2f(
        canvas_pos.x / u.canvas_size.x * 2.0 - 1.0,
        1.0 - canvas_pos.y / u.canvas_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.dab_uv = unit * u.uv_max;
    out.canvas_pos = canvas_pos;
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let dab = textureSample(t_dab, s_dab, in.dab_uv);

    // Selection masking: sample selection texture at canvas UV.
    let sel_uv = in.canvas_pos / u.canvas_size;
    let sel = textureSample(t_selection, s_selection, sel_uv).r;

    return vec4f(dab.rgb, dab.a * sel);
}
