// Circle / rect painting: positions a quad at the paint region, computes
// SDF coverage for circles, applies selection masking.
// Hardware blend state handles alpha compositing — see PaintPipelines for blend config.

struct Uniforms {
    // Quad origin in canvas pixels (top-left corner).
    origin: vec2f,
    // Quad size in canvas pixels.
    size: vec2f,
    // Padded canvas dimensions.
    canvas_size: vec2f,
    // Circle center in canvas pixels.
    center: vec2f,
    // Circle radius in pixels. 0 = solid fill (coverage always 1.0).
    radius: f32,
    // Soft edge width in pixels (typically 1.0).
    softness: f32,
    _pad: vec2f,
    // Paint color (RGBA, straight alpha).
    color: vec4f,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var t_selection: texture_2d<f32>;
@group(1) @binding(1) var t_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle trick: 3 vertices cover the [0,1]² square.
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));

    // Map unit quad to the paint region's canvas-space rectangle.
    let canvas_pos = uniforms.origin + unit * uniforms.size;

    // Canvas pixels → NDC: [0, canvas_size] → [-1, 1], Y flipped.
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
    var coverage = 1.0;

    // Circle SDF when radius > 0.
    if uniforms.radius > 0.0 {
        let dist = distance(in.canvas_pos, uniforms.center);
        coverage = 1.0 - smoothstep(
            uniforms.radius - uniforms.softness,
            uniforms.radius,
            dist,
        );
    }

    // Selection masking: sample selection texture at canvas UV.
    let sel_uv = in.canvas_pos / uniforms.canvas_size;
    let sel = textureSample(t_selection, t_sampler, sel_uv).r;
    coverage *= sel;

    return vec4f(uniforms.color.rgb, uniforms.color.a * coverage);
}
