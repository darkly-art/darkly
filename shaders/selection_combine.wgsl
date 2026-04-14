// Selection boolean ops — combines two R8 selection masks.
//
// Modes: 0=Add, 1=Subtract, 2=Intersect, 3=Invert (ignores shape).
// Uses the shared fullscreen triangle vertex shader.

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

@group(0) @binding(0) var existing_tex: texture_2d<f32>;
@group(0) @binding(1) var shape_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct Params {
    mode: u32,
}
@group(0) @binding(3) var<uniform> params: Params;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let a = textureSample(existing_tex, samp, in.uv).r;
    let b = textureSample(shape_tex, samp, in.uv).r;

    var result: f32;
    switch params.mode {
        case 0u: { result = a + b - a * b; }   // Add (alpha union)
        case 1u: { result = max(0.0, a - b); }  // Subtract
        case 2u: { result = min(a, b); }         // Intersect
        case 3u: { result = 1.0 - a; }           // Invert
        default: { result = b; }
    }

    return vec4f(result, 0.0, 0.0, 1.0);
}
