// Image-rescale resampling.
//
// `fs_resample` maps each destination pixel back to a normalized position
// inside the source's old canvas extent and bilinearly samples it — used for
// upscaling and for the final (≤2x) downscale step. `fs_halve` box-reduces a
// texture to half size; the engine chains it (a box pyramid) so large
// reductions stay anti-aliased, then runs one `fs_resample` to the exact size.
//
// RGBA is resampled in premultiplied alpha (premultiply on load, un-premultiply
// on store) so transparent texels don't bleed colour across edges — the same
// discipline as transform_commit.wgsl. R8 masks are single-channel and straight.
//
// Samples are taken with `textureLoad` (no hardware sampler) so the premultiply
// happens *before* the filter weights are applied.

// Packed into vec4 rows for an unambiguous uniform layout (see the matching
// Rust `Params`):
//   p0 = [new_origin.xy,  canvas_origin.xy]
//   p1 = [inv_scale.xy,   old_origin.xy]
//   p2 = [old_size.xy,    is_r8, premul_io]
// new_origin:    canvas-space offset of the destination texture's (0,0) pixel.
// canvas_origin: the rescale anchor (document scales about it).
// inv_scale:     old_size / new_size — maps a dest canvas point back to source.
// old_origin:    canvas-space offset of the source's old extent (0,0) pixel.
// old_size:      source old-extent dimensions in canvas pixels.
// is_r8:         0.0 = RGBA (premultiplied resample), 1.0 = R8 (straight).
// premul_io:     1.0 = the source texture and the render target both hold
//                premultiplied RGBA, so texels average as-is with no
//                premultiply/un-premultiply round trip. Set when generating a
//                mip chain over a premultiplied source; 0.0 for the
//                straight-alpha layer/mask rescale path.
struct Params {
    p0: vec4f,
    p1: vec4f,
    p2: vec4f,
};

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: Params;

struct VsOut { @builtin(position) pos: vec4f };

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle.
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn premul(c: vec4f) -> vec4f { return vec4f(c.rgb * c.a, c.a); }

fn unpremul(c: vec4f) -> vec4f {
    if (c.a <= 0.0) { return vec4f(0.0); }
    return vec4f(c.rgb / c.a, c.a);
}

// `raw` means "the stored texels are already in a form that averages
// linearly" — either single-channel R8, or RGBA that is already
// premultiplied. Both skip the premultiply/un-premultiply round trip that
// straight-alpha RGBA needs.
fn load_weighted(p: vec2i, dims: vec2i, raw: bool) -> vec4f {
    let c = clamp(p, vec2i(0), dims - vec2i(1));
    let texel = textureLoad(t_src, c, 0);
    if (raw) { return texel; }
    return premul(texel);
}

// Whether this invocation averages texels as-is. `p2.z` flags R8 masks;
// `p2.w` flags RGBA whose source *and* destination are premultiplied (mip
// chain generation over a premultiplied source texture).
fn is_raw() -> bool {
    return u.p2.z > 0.5 || u.p2.w > 0.5;
}

@fragment
fn fs_resample(in: VsOut) -> @location(0) vec4f {
    let new_origin = u.p0.xy;
    let canvas_origin = u.p0.zw;
    let inv_scale = u.p1.xy;
    let old_origin = u.p1.zw;
    let old_size = u.p2.xy;
    let raw = is_raw();

    let dims = vec2i(textureDimensions(t_src));
    let c_new = new_origin + in.pos.xy;
    let c_old = canvas_origin + (c_new - canvas_origin) * inv_scale;
    let uv = (c_old - old_origin) / old_size;     // 0..1 within old extent
    let texel = uv * vec2f(dims) - vec2f(0.5);        // bilinear sample center
    let base = vec2i(floor(texel));
    let f = texel - floor(texel);
    let s00 = load_weighted(base + vec2i(0, 0), dims, raw);
    let s10 = load_weighted(base + vec2i(1, 0), dims, raw);
    let s01 = load_weighted(base + vec2i(0, 1), dims, raw);
    let s11 = load_weighted(base + vec2i(1, 1), dims, raw);
    let top = mix(s00, s10, f.x);
    let bot = mix(s01, s11, f.x);
    let p = mix(top, bot, f.y);
    if (raw) { return p; }
    return unpremul(p);
}

@fragment
fn fs_halve(in: VsOut) -> @location(0) vec4f {
    let raw = is_raw();
    let dims = vec2i(textureDimensions(t_src));
    let dst = vec2i(floor(in.pos.xy));
    let src = dst * 2;
    let s00 = load_weighted(src + vec2i(0, 0), dims, raw);
    let s10 = load_weighted(src + vec2i(1, 0), dims, raw);
    let s01 = load_weighted(src + vec2i(0, 1), dims, raw);
    let s11 = load_weighted(src + vec2i(1, 1), dims, raw);
    let p = (s00 + s10 + s01 + s11) * 0.25;
    if (raw) { return p; }
    return unpremul(p);
}
