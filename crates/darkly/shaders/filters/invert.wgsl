// Invert-colors adjustment: `1 - rgb`, alpha preserved.
//
// The `invert_color` atom is supplied by `shaders/lib/color.wgsl`, which the
// Rust side (`gpu/adjustments/invert.rs`) `include_str!`-prepends, so this
// file owns only the per-pixel plumbing, never the color math. Exact per-texel
// `textureLoad` (no sampler), so the result is bit-exact up to the inversion.
//
// Two entry points share that math:
//   fs_invert        - every texel inverted. Used over a whole node, or where
//                      there's no selection.
//   fs_invert_masked - inverts where the R8 selection mask is selected (>0.5),
//                      passes the original through elsewhere, so a non-
//                      rectangular selection clips exactly (mirrors the ortho
//                      pass's `fs_mirror_masked`).
//
// One entry-point pair serves RGBA8 layers and R8 masks: the pipeline target
// format is the only difference (an R8 target stores `1 - r`).

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

@group(0) @binding(1) var t_mask: texture_2d<f32>;

@fragment
fn fs_invert_masked(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let orig = textureLoad(t_src, p, 0);
    let inv = invert_color(orig);
    let selected = textureLoad(t_mask, p, 0).r > 0.5;
    return select(orig, inv, selected);
}
