// Invert-colors adjustment: `1 - rgb`, alpha preserved.
//
// The `invert_color` atom is supplied by `shaders/lib/color.wgsl`, which the
// Rust side (`gpu/effects/invert.rs`) `include_str!`-prepends — so this
// file owns only the per-pixel plumbing, never the color math. Exact per-texel
// `textureLoad` (no sampler), so the result is bit-exact up to the inversion.
//
// One entry point serves RGBA8 layers and R8 masks alike — the pipeline target
// format is the only difference (an R8 target stores `1 - r`). Where the
// inversion lands is not this shader's concern: a mask or a selection confines
// it from outside, in the shared in-place apply pass.

@group(0) @binding(0) var t_src: texture_2d<f32>;

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
fn fs_invert(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    return invert_color(textureLoad(t_src, p, 0));
}
