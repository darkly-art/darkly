// Per-channel tone-curve adjustment via a precomputed 256×1 RGBA8 LUT.
//
// The LUT is baked on the CPU (`gpu/filters/curves.rs`) so each texel already
// folds the per-channel curve and the composite "value" curve together —
// `lut.rgb[i] = value(channel(i))`, `lut.a[i] = alpha(i)` (the value curve is
// not applied to alpha; per GIMP `gimp_curve_map_pixels`). The shader is then a
// single `textureLoad` per channel at the integer index `round(c*255)` — no
// sampler, no branching, so identity curves are exactly identity.

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var t_lut: texture_2d<f32>;

struct VsOut { @builtin(position) pos: vec4f };

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle.
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn lut_index(c: f32) -> i32 {
    return i32(round(clamp(c, 0.0, 1.0) * 255.0));
}

@fragment
fn fs_curves(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let src = textureLoad(t_src, p, 0);
    // Each channel indexes the LUT at its own stored value and reads its own
    // baked component.
    let r = textureLoad(t_lut, vec2i(lut_index(src.r), 0), 0).r;
    let g = textureLoad(t_lut, vec2i(lut_index(src.g), 0), 0).g;
    let b = textureLoad(t_lut, vec2i(lut_index(src.b), 0), 0).b;
    let a = textureLoad(t_lut, vec2i(lut_index(src.a), 0), 0).a;
    return vec4f(r, g, b, a);
}
