// Selection morphology — one dilate (max) / erode (min) step over an R8 mask.
//
// The engine runs N single-pixel steps through the ping-pong textures,
// alternating 4-/8-connected neighbourhoods each step so the structuring
// element approximates a disc (octagon) rather than a square. Grow, shrink,
// border, and smooth are all built from this primitive.

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

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct Params {
    op: u32,           // 0 = dilate (max), 1 = erode (min)
    neighborhood: u32, // 0 = 4-connected, 1 = 8-connected
    texel: vec2f,      // 1 / texture size
}
@group(0) @binding(2) var<uniform> params: Params;

fn fold(acc: f32, v: f32, op: u32) -> f32 {
    if op == 0u { return max(acc, v); }
    return min(acc, v);
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let t = params.texel;
    let op = params.op;
    var acc = textureSample(src_tex, samp, in.uv).r;
    acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(-t.x, 0.0)).r, op);
    acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(t.x, 0.0)).r, op);
    acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(0.0, -t.y)).r, op);
    acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(0.0, t.y)).r, op);
    if params.neighborhood == 1u {
        acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(-t.x, -t.y)).r, op);
        acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(t.x, -t.y)).r, op);
        acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(-t.x, t.y)).r, op);
        acc = fold(acc, textureSample(src_tex, samp, in.uv + vec2f(t.x, t.y)).r, op);
    }
    return vec4f(acc, 0.0, 0.0, 1.0);
}
