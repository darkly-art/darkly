// Final blit from accumulator to surface with view transform.

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}

struct ViewTransform {
    row0: vec4f,
    row1: vec4f,
    row2: vec4f,
}

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> view: ViewTransform;

@fragment fn fs_present(in: VertexOutput) -> @location(0) vec4f {
    // Transform screen pixel -> canvas pixel using the inverse view matrix
    let screen_pos = in.position.xy;
    let canvas_x = view.row0.x * screen_pos.x + view.row1.x * screen_pos.y + view.row2.x;
    let canvas_y = view.row0.y * screen_pos.x + view.row1.y * screen_pos.y + view.row2.y;

    let dims = vec2f(textureDimensions(t_source));
    let uv = vec2f(canvas_x, canvas_y) / dims;

    // Clamp UV to [0,1] for the sample (textureSample requires uniform control flow)
    let clamped_uv = clamp(uv, vec2f(0.0), vec2f(1.0));
    let color = textureSample(t_source, t_sampler, clamped_uv);

    // Out-of-bounds -> workspace background color
    let oob = uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0;
    let bg = vec4f(0.11, 0.11, 0.11, 1.0);
    return select(vec4f(color.rgb, 1.0), bg, oob);
}
