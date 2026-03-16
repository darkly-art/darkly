// Linear gradient: fullscreen quad with per-pixel gradient interpolation.
// Selection masking via bound selection texture.
// Hardware blend state = REPLACE (gradient overwrites existing content,
// modulated by selection coverage).

struct Uniforms {
    // Quad origin in canvas pixels (top-left corner).
    origin: vec2f,
    // Quad size in canvas pixels.
    size: vec2f,
    // Padded canvas dimensions.
    canvas_size: vec2f,
    // Gradient start point in canvas pixels.
    start: vec2f,
    // Gradient end point in canvas pixels.
    end: vec2f,
    _pad: vec2f,
    // Start color (RGBA, straight alpha).
    color0: vec4f,
    // End color (RGBA, straight alpha).
    color1: vec4f,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var t_selection: texture_2d<f32>;
@group(1) @binding(1) var t_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    let canvas_pos = uniforms.origin + unit * uniforms.size;

    let ndc = vec2f(
        canvas_pos.x / uniforms.canvas_size.x * 2.0 - 1.0,
        1.0 - canvas_pos.y / uniforms.canvas_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.canvas_pos = canvas_pos;
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Project pixel onto gradient axis.
    let d = uniforms.end - uniforms.start;
    let len2 = dot(d, d);
    var t = 0.0;
    if len2 > 0.001 {
        t = clamp(dot(in.canvas_pos - uniforms.start, d) / len2, 0.0, 1.0);
    }

    let color = mix(uniforms.color0, uniforms.color1, t);

    // Selection masking.
    let sel_uv = in.canvas_pos / uniforms.canvas_size;
    let sel = textureSample(t_selection, t_sampler, sel_uv).r;

    return vec4f(color.rgb, color.a * sel);
}
