// VHS tape post-processing veil.
// Ported from https://www.shadertoy.com/view/XtBXDt by FMS_Cat
// Simulates tape wobble, tape crease tears, head-switching noise band,
// chromatic RGB bloom, and rolling AC-beat brightness modulation.

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

struct Params {
    time: f32,
    wobble: f32,
    switching: f32,
    bloom: f32,
    ac_beat: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

const PI: f32 = 3.14159265;

// Sample the input, returning a flat gray for out-of-bounds x so that
// horizontal wobble pushing off-screen shows a VHS-style border rather
// than a clamp-stretch smear. The fragment shader works in Shadertoy
// y-up space; flip y back here so the texture sample hits the right
// pixel under our Y-down input convention.
fn tex2d(uv: vec2f) -> vec3f {
    if (abs(uv.x - 0.5) > 0.5) {
        return vec3f(0.1);
    }
    return textureSampleLevel(t_input, t_sampler, vec2f(uv.x, 1.0 - uv.y), 0.0).rgb;
}

fn hash2(v: vec2f) -> f32 {
    return fract(sin(dot(v, vec2f(89.44, 19.36))) * 22189.22);
}

fn i_hash(v: vec2f, r: vec2f) -> f32 {
    let h00 = hash2(floor(v * r + vec2f(0.0, 0.0)) / r);
    let h10 = hash2(floor(v * r + vec2f(1.0, 0.0)) / r);
    let h01 = hash2(floor(v * r + vec2f(0.0, 1.0)) / r);
    let h11 = hash2(floor(v * r + vec2f(1.0, 1.0)) / r);
    let f = fract(v * r);
    let ip = smoothstep(vec2f(0.0), vec2f(1.0), f);
    return (h00 * (1.0 - ip.x) + h10 * ip.x) * (1.0 - ip.y)
         + (h01 * (1.0 - ip.x) + h11 * ip.x) * ip.y;
}

fn noise2(v: vec2f) -> f32 {
    var sum = 0.0;
    for (var i = 1; i < 9; i = i + 1) {
        let fi = f32(i);
        let scale = 2.0 * pow(2.0, fi);
        sum += i_hash(v + vec2f(fi), vec2f(scale)) / pow(2.0, fi);
    }
    return sum;
}

@fragment fn fs_vhs(in: VertexOutput) -> @location(0) vec4f {
    // Shadertoy convention: y=0 at bottom. Our vertex shader produces a
    // flipped UV (y=0 at visual top), so flip back for the shader body
    // so the switching-noise band lands at the visual bottom.
    let uv = vec2f(in.uv.x, 1.0 - in.uv.y);
    var uvn = uv;
    let t = params.time;

    // tape wave
    uvn.x += (noise2(vec2f(uvn.y, t)) - 0.5) * 0.005 * params.wobble;
    uvn.x += (noise2(vec2f(uvn.y * 100.0, t * 10.0)) - 0.5) * 0.01 * params.wobble;

    // tape crease
    let tc_phase = clamp(
        (sin(uvn.y * 8.0 - t * PI * 1.2) - 0.92) * noise2(vec2f(t)),
        0.0,
        0.01,
    ) * 10.0 * params.wobble;
    let tc_noise = max(noise2(vec2f(uvn.y * 100.0, t * 10.0)) - 0.5, 0.0);
    uvn.x = uvn.x - tc_noise * tc_phase;

    // switching noise
    let sn_phase = smoothstep(0.03, 0.0, uvn.y) * params.switching;
    uvn.y += sn_phase * 0.3;
    uvn.x += sn_phase * ((noise2(vec2f(uv.y * 100.0, t * 10.0)) - 0.5) * 0.2);

    var col = tex2d(uvn);
    col *= 1.0 - tc_phase;
    col = mix(col, col.yzx, sn_phase);

    // chromatic bloom (RGB offset sweep)
    var x = -4.0;
    loop {
        if (x >= 2.5) { break; }
        col.x += tex2d(uvn + vec2f(x - 0.0, 0.0) * 7e-3).x * 0.1 * params.bloom;
        col.y += tex2d(uvn + vec2f(x - 2.0, 0.0) * 7e-3).y * 0.1 * params.bloom;
        col.z += tex2d(uvn + vec2f(x - 4.0, 0.0) * 7e-3).z * 0.1 * params.bloom;
        x = x + 1.0;
    }
    col *= 0.6;

    // ac beat
    let beat = clamp(noise2(vec2f(0.0, uv.y + t * 0.2)) * 0.6 - 0.25, 0.0, 0.1);
    col *= 1.0 + beat * params.ac_beat;

    return vec4f(col, 1.0);
}
