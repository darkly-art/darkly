// Rainy glass post-processing veil.
// Ported from "Heartfelt" by Martijn Steinrucken (BigWings) — Shadertoy.
//   https://www.shadertoy.com/view/ltffzl
// License: Creative Commons Attribution-NonCommercial-ShareAlike 3.0 Unported.
// Stripped to core rain effect: drops, trails, fog distortion.

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
    rain_amount: f32,
    resolution_x: f32,
    resolution_y: f32,
    direction: f32,
}

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

fn rotate2d(v: vec2f, angle: f32) -> vec2f {
    let c = cos(angle);
    let s = sin(angle);
    return vec2f(v.x * c - v.y * s, v.x * s + v.y * c);
}

// --- Noise / hash functions ---

fn N13(p: f32) -> vec3f {
    var p3 = fract(vec3f(p) * vec3f(0.1031, 0.11369, 0.13787));
    p3 += dot(p3, p3.yzx + 19.19);
    return fract(vec3f((p3.x + p3.y) * p3.z, (p3.x + p3.z) * p3.y, (p3.y + p3.z) * p3.x));
}

fn N1(t: f32) -> f32 {
    return fract(sin(t * 12345.564) * 7658.76);
}

fn Saw(b: f32, t: f32) -> f32 {
    return smoothstep(0.0, b, t) * smoothstep(1.0, b, t);
}

// --- Drop layer: falling drops with trails ---

fn DropLayer2(uv_in: vec2f, t: f32) -> vec2f {
    let UV = uv_in;
    var uv = uv_in;

    uv.y += t * 0.75;
    let a = vec2f(6.0, 1.0);
    let grid = a * 2.0;
    var id = floor(uv * grid);

    let colShift = N1(id.x);
    uv.y += colShift;

    id = floor(uv * grid);
    let n = N13(id.x * 35.2 + id.y * 2376.1);
    let st = fract(uv * grid) - vec2f(0.5, 0.0);

    var x = n.x - 0.5;

    let y_wave = UV.y * 20.0;
    let wiggle = sin(y_wave + sin(y_wave));
    x += wiggle * (0.5 - abs(x)) * (n.z - 0.5);
    x *= 0.7;
    let ti = fract(t + n.z);
    let y_drop = (Saw(0.85, ti) - 0.5) * 0.9 + 0.5;
    let p = vec2f(x, y_drop);

    let d = length((st - p) * a.yx);

    let mainDrop = smoothstep(0.4, 0.0, d);

    let r = sqrt(smoothstep(1.0, y_drop, st.y));
    let cd = abs(st.x - x);
    var trail = smoothstep(0.23 * r, 0.15 * r * r, cd);
    let trailFront = smoothstep(-0.02, 0.02, st.y - y_drop);
    trail *= trailFront * r * r;

    let y_uv = UV.y;
    let trail2 = smoothstep(0.2 * r, 0.0, cd);
    let y_frac = fract(y_uv * 10.0) + (st.y - 0.5);
    let dd = length(st - vec2f(x, y_frac));
    let droplets = smoothstep(0.3, 0.0, dd);
    let m = mainDrop + droplets * r * trailFront;

    return vec2f(m, trail);
}

// --- Static drops (small stationary beads) ---

fn StaticDrops(uv_in: vec2f, t: f32) -> f32 {
    let uv_scaled = uv_in * 40.0;

    let id = floor(uv_scaled);
    let uv = fract(uv_scaled) - 0.5;
    let n = N13(id.x * 107.45 + id.y * 3543.654);
    let p = (n.xy - 0.5) * 0.7;
    let d = length(uv - p);

    let fade = Saw(0.025, fract(t + n.z));
    let c = smoothstep(0.3, 0.0, d) * fract(n.z * 10.0) * fade;
    return c;
}

// --- Composite drops from multiple layers ---

fn Drops(uv: vec2f, t: f32, l0: f32, l1: f32, l2: f32) -> vec2f {
    let s = StaticDrops(uv, t) * l0;
    let m1 = DropLayer2(uv, t) * l1;
    let m2 = DropLayer2(uv * 1.85, t) * l2;

    let c = smoothstep(0.3, 1.0, s + m1.x + m2.x);

    return vec2f(c, max(m1.y * l0, m2.y * l1));
}

@fragment fn fs_rainy_glass(in: VertexOutput) -> @location(0) vec4f {
    let aspect = params.resolution_x / params.resolution_y;
    let dir = params.direction;

    // Map UVs to centered coordinates scaled by aspect ratio,
    // then rotate into rain-space so the pattern falls in the
    // configured direction.
    var uv = (in.uv - 0.5) * vec2f(aspect, 1.0);
    uv = rotate2d(uv, dir);

    let UV = in.uv;
    let t = params.time * 0.2;

    let rainAmount = params.rain_amount;

    let staticDrops = smoothstep(-0.5, 1.0, rainAmount) * 2.0;
    let layer1 = smoothstep(0.25, 0.75, rainAmount);
    let layer2 = smoothstep(0.0, 0.5, rainAmount);

    let c = Drops(uv, t, staticDrops, layer1, layer2);

    // Compute normals from the drop pattern (in rain-space)
    let e = vec2f(0.001, 0.0);
    let cx = Drops(uv + e, t, staticDrops, layer1, layer2).x;
    let cy = Drops(uv + e.yx, t, staticDrops, layer1, layer2).x;
    let n_rain = vec2f(cx - c.x, cy - c.x);

    // Rotate normals back to screen-space for correct distortion
    let n = rotate2d(n_rain, -dir);

    // Sample input with distortion from rain normals
    let col = textureSample(t_input, t_sampler, UV + n);

    return col;
}
