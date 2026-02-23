// Final blit from accumulator to surface.

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

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

@fragment fn fs_present(in: VertexOutput) -> @location(0) vec4f {
    // Derive UV from pixel position so that surface pixel (x,y) reads
    // texture pixel (x,y). This avoids squishing when the composite cache
    // is padded to tile boundaries and larger than the surface.
    let uv = in.position.xy / vec2f(textureDimensions(t_source));
    let color = textureSample(t_source, t_sampler, uv);
    return vec4f(color.rgb, 1.0);
}
