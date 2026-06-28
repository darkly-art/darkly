// Shared inverse-homography sampler. The single home for "map a destination /
// window-local fragment back through an inverse 3×3 homography, with the
// per-pixel perspective divide". Concatenated ahead of every consumer shader
// at pipeline-build time (`include_str!` / `concat!`), the same mechanism
// `lib/fbm.wgsl` and `lib/color.wgsl` use — so the floating commit path
// (`transform_commit.wgsl`) and the voids (`voids/video_stream.wgsl`,
// `voids/noise.wgsl`) share one implementation instead of three affine copies.
//
// The three rows are an inverse matrix packed `[m00,m01,m02,_], [m10,m11,m12,_],
// [m20,m21,m22,_]` (see `gpu::transform::pack_inv_rows`). Affine is the special
// case `inv2 == [0,0,1,_]`, where `h.z ≡ 1` and the divide is a no-op.
//
// Returns `vec3f(pre.xy, ok)`: `pre` is the pre-transform position (still in the
// caller's pixel frame — each consumer owns its own normalization to a source
// UV), and `ok` is 1.0 when the sample is valid or 0.0 when the homogeneous w
// collapsed (a corner folded behind the camera) so the caller can early-out
// transparent / clamp.
fn proj_local(inv0: vec4f, inv1: vec4f, inv2: vec4f, p: vec2f) -> vec3f {
    let hx = inv0.x * p.x + inv0.y * p.y + inv0.z;
    let hy = inv1.x * p.x + inv1.y * p.y + inv1.z;
    let hw = inv2.x * p.x + inv2.y * p.y + inv2.z;
    if (abs(hw) < 1e-8) {
        return vec3f(0.0, 0.0, 0.0);
    }
    return vec3f(hx / hw, hy / hw, 1.0);
}
