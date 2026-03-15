// Dab compositing: positions a quad at the dab's canvas-space rectangle
// and samples the dab texture with alpha modulation (opacity × flow).
// Hardware blend state handles alpha-over — no shader-side blend logic.

struct Uniforms {
    // Dab position in canvas pixels (top-left corner).
    dab_origin: vec2f,
    // Dab size in canvas pixels.
    dab_size: vec2f,
    // Canvas size in pixels (padded to tile boundary).
    canvas_size: vec2f,
    // Alpha modulation: opacity × flow.
    alpha_mod: f32,
    _pad: f32,
}

@group(0) @binding(0) var t_dab: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle trick: generate a unit quad from vertex index.
    // idx: 0 → (0,0), 1 → (2,0), 2 → (0,2) — covers the [0,1]² square.
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));

    // Map unit quad to the dab's canvas-space rectangle.
    let canvas_pos = uniforms.dab_origin + unit * uniforms.dab_size;

    // Convert canvas pixels → NDC: [0, canvas_size] → [-1, 1], Y flipped.
    let ndc = vec2f(
        canvas_pos.x / uniforms.canvas_size.x * 2.0 - 1.0,
        1.0 - canvas_pos.y / uniforms.canvas_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.uv = unit;
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let dab = textureSample(t_dab, t_sampler, in.uv);
    return vec4f(dab.rgb, dab.a * uniforms.alpha_mod);
}
