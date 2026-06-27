// Orthogonal (90°-multiple) transform of a texture: exact pixel permutation.
//
// Flips and 90° rotations relabel texels without resampling, so every fetch is
// a `textureLoad` at an integer source index — no sampler, no filtering, no
// premultiply. Output is bit-identical to the input up to the permutation,
// which is the whole point (cf. rescale.wgsl, which *does* resample).
//
// Two entry points share the same inverse index map:
//   fs_remap         — whole-surface transform; dest may be a rotated (swapped)
//                      size. dest texel ← src[inv_map(dest)]. Used by canvas
//                      flip/rotate (per node) and the selection mask.
//   fs_mirror_masked — in-place H/V mirror of a region about its own centre,
//                      gated by an R8 mask: selected texels take the mirror,
//                      unselected texels pass through unchanged. Used by layer/
//                      selection flip so a non-rectangular selection clips
//                      correctly. (Mirror only — dest size == src size.)
//
// xform codes (match the Rust `OrthoXform` discriminants):
//   0 FlipH   1 FlipV   2 Rot180   3 Rot90Cw   4 Rot90Ccw

struct Params {
    // [xform, src_w, src_h, has_mask]
    p: vec4u,
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

// Inverse map: given a destination texel index and the source dimensions,
// return the source texel index that lands on it. Derived by inverting the
// forward local-coordinate map in `OrthoXform::map_local`.
fn inv_map(dst: vec2i, sw: i32, sh: i32) -> vec2i {
    let xform = u.p.x;
    switch xform {
        case 0u: { return vec2i(sw - 1 - dst.x, dst.y); }          // FlipH
        case 1u: { return vec2i(dst.x, sh - 1 - dst.y); }          // FlipV
        case 2u: { return vec2i(sw - 1 - dst.x, sh - 1 - dst.y); } // Rot180
        case 3u: { return vec2i(dst.y, sh - 1 - dst.x); }          // Rot90Cw  (dest is sh×sw)
        default: { return vec2i(sw - 1 - dst.y, dst.x); }          // Rot90Ccw (dest is sh×sw)
    }
}

@fragment
fn fs_remap(in: VsOut) -> @location(0) vec4f {
    let sw = i32(u.p.y);
    let sh = i32(u.p.z);
    let dst = vec2i(floor(in.pos.xy));
    return textureLoad(t_src, inv_map(dst, sw, sh), 0);
}

@group(0) @binding(2) var t_mask: texture_2d<f32>;

@fragment
fn fs_mirror_masked(in: VsOut) -> @location(0) vec4f {
    let sw = i32(u.p.y);
    let sh = i32(u.p.z);
    let dst = vec2i(floor(in.pos.xy));
    // fs_mirror_masked is only invoked with FlipH/FlipV, so dest size == src.
    let mirror = inv_map(dst, sw, sh);
    let selected = textureLoad(t_mask, dst, 0).r > 0.5;
    let src = select(dst, mirror, selected);
    return textureLoad(t_src, src, 0);
}
