// Chromatic aberration filter (destructive apply + filter layer). The shared
// per-pixel math lives in `lib/aberration.wgsl`, prepended at load time by
// gpu/filters/chromatic_aberration.rs (the render shaders have no #include).
//
// Bilinear source mode: `src` is filterable + a `Filtering` sampler at binding
// 1, so the ghost/blur taps read fractional offsets.

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var t_samp: sampler;
@group(0) @binding(2) var<uniform> params: AberrationParams;

struct VsOut { @builtin(position) pos: vec4f };

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle.
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_ca(in: VsOut) -> @location(0) vec4f {
    let uv = in.pos.xy / vec2f(textureDimensions(t_src));
    return aberration_apply(t_src, t_samp, uv, params);
}

// Destructive selection-clipped variant: aberrate where the R8 mask is selected
// (>0.5), pass the original texel through elsewhere (mirrors `fs_bc_masked`).
@group(0) @binding(3) var t_mask: texture_2d<f32>;

@fragment
fn fs_ca_masked(in: VsOut) -> @location(0) vec4f {
    let uv = in.pos.xy / vec2f(textureDimensions(t_src));
    let filtered = aberration_apply(t_src, t_samp, uv, params);
    let p = vec2i(floor(in.pos.xy));
    let orig = textureLoad(t_src, p, 0);
    let selected = textureLoad(t_mask, p, 0).r > 0.5;
    return select(orig, filtered, selected);
}
