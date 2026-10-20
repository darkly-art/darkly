// The paint terminal's accumulation laws, applied per dab against the live
// stroke ground by the dispatch-per-dab skeleton. `src` is the dab's
// premultiplied deposit at the pixel (shape, flow, selection and the dial's
// share already folded in); `dst` is the ground texel, premultiplied.
//
// Emitted into a brush's module by `paint::compile_wgsl` through
// `NodeWgsl::decls`, after `lib/deposit_ceiling.wgsl`, which supplies
// `ceiling_t`. Only brushes whose terminal is `paint` carry it.

// Build-up: premultiplied source-over. Coverage accumulates as
// 1 - prod(1 - a_i); a pixel's density rises with every dab that lands.
fn accumulate_build(src: vec4f, dst: vec4f) -> vec4f {
    return src + dst * (1.0 - src.a);
}

// Move: a lerp toward the dab by the finger's coverage `cov`, which is
// independent of the dab's alpha. What the finger brought (`src`,
// premultiplied, which may be transparent where it came from bare paper)
// replaces `cov` of what was there; the same `cov` accumulates in the
// coverage channel through `accumulate_build` on a bare alpha, so the
// commit can lay the moved pigment over `1 - cov` of the pre-stroke layer.
//
// `dst` and the result are *straight* alpha, unlike the other two laws'
// grounds. A soft tip brings a few percent of coverage per dab at its
// edge, and a dark colour times that coverage rounds to zero in a
// premultiplied 8-bit texel while the alpha does not: the pigment the
// finger carries would turn black and darken every edge it touches.
// Straight storage keeps the colour exact and quantises only the weights.
fn accumulate_move(src: vec4f, cov: f32, dst: vec4f) -> vec4f {
    let pre = src + vec4f(dst.rgb * dst.a, dst.a) * (1.0 - cov);
    if pre.a <= 0.0 {
        return vec4f(0.0);
    }
    return vec4f(pre.rgb / pre.a, pre.a);
}

// Wash: the deposit ceiling applied per dab against the live ground. For
// one pigment this is exactly the greatest coverage any dab laid on the
// pixel (docs/brush/architecture.md, "How dabs accumulate in the
// scratch"); for varying pigments it deposits whole colours instead of
// per-channel maxima. A pixel the ceiling leaves no room on is returned
// untouched, so a dab no stronger than what is already there changes
// nothing, bit for bit.
fn accumulate_wash(src: vec4f, dst: vec4f) -> vec4f {
    let t = ceiling_t(src, dst);
    if t <= 0.0 {
        return dst;
    }
    return vec4f(src.rgb / src.a * t, t) + dst * (1.0 - t);
}
