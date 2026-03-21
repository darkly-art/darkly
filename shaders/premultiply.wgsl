// Straight-alpha to premultiplied-alpha conversion.
//
// Reads each texel via textureLoad (no filtering), multiplies RGB by alpha,
// and outputs premultiplied values with REPLACE blend. Used after GPU→GPU
// texture copies from straight-alpha layer textures to ensure the transform
// source texture is premultiplied for correct bilinear interpolation.

struct VertexOutput {
    @builtin(position) position: vec4f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var t_input: texture_2d<f32>;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let coord = vec2i(in.position.xy);
    let c = textureLoad(t_input, coord, 0);
    return vec4f(c.rgb * c.a, c.a);
}
