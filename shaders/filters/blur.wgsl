// Separable Gaussian blur — run once for horizontal, once for vertical.

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

struct BlurParams {
    radius: f32,
    direction_x: f32,
    direction_y: f32,
    _pad: f32,
}
@group(0) @binding(2) var<uniform> params: BlurParams;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@fragment fn fs_blur(in: VertexOutput) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(t_input));
    let direction = vec2f(params.direction_x, params.direction_y);
    let step = direction / dims;
    var color = vec4f(0.0);
    var weight_sum = 0.0;
    let r = i32(params.radius);
    for (var i = -r; i <= r; i++) {
        let w = 1.0 - abs(f32(i)) / (params.radius + 1.0); // triangle kernel
        color += textureSample(t_input, t_sampler, in.uv + step * f32(i)) * w;
        weight_sum += w;
    }
    return color / weight_sum;
}
