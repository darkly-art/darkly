// Black and White filter: the textureLoad wrapper around the shared
// `bw_transform` core (`shaders/lib/black_and_white.wgsl`, prepended at load
// time by `gpu/filters/black_and_white.rs`). Alpha passes through; no output
// clamp, since the unorm render target clamps on store.

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: BwParams;

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
fn fs_black_and_white(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let c = textureLoad(t_src, p, 0);
    return vec4f(bw_transform(c.rgb, params), c.a);
}

// Destructive selection-clipped variant: transform where the R8 mask is
// selected (>0.5), pass the original through elsewhere (mirrors
// `fs_invert_masked`).
@group(0) @binding(2) var t_mask: texture_2d<f32>;

@fragment
fn fs_black_and_white_masked(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let orig = textureLoad(t_src, p, 0);
    let filtered = vec4f(bw_transform(orig.rgb, params), orig.a);
    let selected = textureLoad(t_mask, p, 0).r > 0.5;
    return select(orig, filtered, selected);
}
