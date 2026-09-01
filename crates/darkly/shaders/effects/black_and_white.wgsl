// Black and White effect — the textureLoad wrapper around the shared
// `bw_transform` core (`shaders/lib/black_and_white.wgsl`, prepended at load
// time by `gpu/effects/black_and_white.rs`). Alpha passes through; no output
// clamp — the unorm render target clamps on store.

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
