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
    fog_amount: f32,
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

fn hash_u32(n: u32) -> u32 {
    var x = n;
    x ^= x >> 16u;
    x *= 0x45d9f3bu;
    x ^= x >> 16u;
    x *= 0x45d9f3bu;
    x ^= x >> 16u;
    return x;
}

fn N13(ix: f32, iy: f32) -> vec3f {
    let h = hash_u32(
        (bitcast<u32>(i32(ix)) * 73856093u) ^
        (bitcast<u32>(i32(iy)) * 19349663u)
    );
    let h2 = hash_u32(h + 1u);
    return vec3f(
        f32(h & 0xFFFFu) / 65535.0,
        f32((h >> 16u) & 0xFFFFu) / 65535.0,
        f32(h2 & 0xFFFFu) / 65535.0,
    );
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
    let n = N13(id.x, id.y);
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
    let n = N13(id.x, id.y);
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

// 13-tap poisson-disk blur approximating the LOD-based fog from the
// original Shadertoy "Heartfelt" shader. When radius is 0 all samples
// collapse to the same texel — effectively a no-op.
fn sample_fog(uv: vec2f, radius: f32, aspect: f32) -> vec3f {
    let offsets = array<vec2f, 12>(
        vec2f(-0.326, -0.406),
        vec2f(-0.840, -0.074),
        vec2f(-0.696,  0.457),
        vec2f(-0.203,  0.621),
        vec2f( 0.962, -0.195),
        vec2f( 0.473, -0.480),
        vec2f( 0.519,  0.767),
        vec2f( 0.185, -0.893),
        vec2f( 0.507,  0.064),
        vec2f( 0.896,  0.412),
        vec2f(-0.322, -0.933),
        vec2f(-0.792, -0.598),
    );

    // Correct for aspect ratio so the blur is circular in screen-space.
    let r = vec2f(radius / aspect, radius);

    var col = textureSampleLevel(t_input, t_sampler, uv, 0.0).rgb;
    for (var i = 0u; i < 12u; i++) {
        col += textureSampleLevel(t_input, t_sampler, uv + offsets[i] * r, 0.0).rgb;
    }
    return col / 13.0;
}

@fragment fn fs_rainy_glass(in: VertexOutput) -> @location(0) vec4f {
    let scale = max(params.resolution_x, params.resolution_y);
    let aspect = params.resolution_x / params.resolution_y;
    let dir = params.direction;

    // Map UVs to centered coordinates normalized by the larger dimension,
    // so the drop density stays stable regardless of aspect ratio.
    // Then rotate into rain-space so the pattern falls in the
    // configured direction.
    var uv = (in.uv - 0.5) * vec2f(params.resolution_x / scale, params.resolution_y / scale);
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

    // Fog: blur the background proportional to fog_amount. Drops see
    // through clearly (minBlur) while the rest of the glass is foggy
    // (maxBlur). Trails cut through the fog where drops have passed.
    let fog = params.fog_amount;
    let maxBlur = fog * 6.0;
    let minBlur = fog * 2.0;
    let focus = max(0.0, mix(maxBlur - c.y, minBlur, smoothstep(0.1, 0.2, c.x)));
    // Exponential radius mapping to match the original's textureLod behavior
    // where each LOD level doubles the blur. This makes the perceptual
    // difference between foggy regions and trail-cleared regions much larger.
    let blur_radius = (pow(2.0, focus) - 1.0) / 2048.0;
    let col = sample_fog(UV + n, blur_radius, aspect);

    return vec4f(col, 1.0);
}
