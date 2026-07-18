// Chromatic aberration — shared per-pixel math for all three effect surfaces
// (destructive filter, filter layer, and veil). Works in UV space using the
// bound texture's own dimensions, so it prepends onto shaders whose binding
// layouts differ: the filter binds `src` at 0, the veil binds `t_input` at 0.
// It never names a global binding — every texture/sampler comes in as a fn
// param (the same technique `lib/canvas.wgsl`'s `sample_mask_window` uses).
// No entry points → tests/shader_compile.rs validates it as a preamble.
//
// The blur's golden-angle spiral tap distribution follows the same
// sunflower/Fibonacci construction as veils/lens_blur.wgsl.

// Array size must match `MAX_ABERRATIONS` in gpu/filters/chromatic_aberration.rs
// — the standard Rust↔WGSL uniform-layout contract.
const CA_MAX: u32 = 16u;
const CA_EPS: f32 = 1e-4;
// Golden angle ≈ 137.508° ≈ 2.39996 rad.
const CA_GA_COS: f32 = -0.7373688;
const CA_GA_SIN: f32 = 0.6754904;

// One aberration entry. Layout mirrors `GpuAberration` (48 B): a vec2 offset,
// two scalars, then a vec3 `axis` at offset 16 (aligned) with `k1`, then `k2`
// (+ trailing pad). `axis` is the entry color's hue direction — the red axis
// rotated about the gray diagonal so the color's hue lands on it (see the CPU
// packing in gpu/filters/chromatic_aberration.rs); `k1`/`k2` split the entry's
// strength between an achromatic full-pixel shift and a chromatic axis shift.
struct Aberration {
    offset_px: vec2f,
    scale: f32,
    blur_px: f32,
    axis: vec3f,
    k1: f32,
    k2: f32,
}

// The whole effect: a live entry count and the fixed-size entry array. Mirrors
// `GpuAberrationParams` (784 B).
struct AberrationParams {
    count: u32,
    entries: array<Aberration, 16>,
}

// Sample at UV, returning transparent (`vec4f(0)`) outside [0,1] so edge content
// doesn't streak outward where users expect transparency (Step 2 boundary
// policy). Straight-alpha in, straight-alpha out. Explicit-LOD sample so it is
// valid in any control flow.
fn aberration_sample_uv(tex: texture_2d<f32>, samp: sampler, uv: vec2f) -> vec4f {
    let inside = all(uv >= vec2f(0.0)) && all(uv <= vec2f(1.0));
    let s = textureSampleLevel(tex, samp, uv, 0.0);
    return select(vec4f(0.0), s, inside);
}

// Blurred sample around `uv`: a golden-angle spiral disk of radius `blur_px`.
// Taps are premultiplied (`rgb·a`) before averaging and un-premultiplied by the
// accumulated alpha at the end — layer textures are straight-alpha, so a
// transparent tap's undefined RGB must not bleed into edges. Below ~0.25 px it
// collapses to a single tap; otherwise the tap count scales with the radius so
// large radii don't band.
fn aberration_blurred(
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2f,
    blur_px: f32,
    dims: vec2f,
) -> vec4f {
    if (blur_px < 0.25) {
        return aberration_sample_uv(tex, samp, uv);
    }
    // ≈ 8 taps per px, clamped — the schema caps blur at 6 px (≤ 48 taps).
    let taps = clamp(i32(ceil(blur_px * 8.0)), 8, 64);
    var dir = vec2f(1.0, 0.0);
    var acc_rgb = vec3f(0.0);
    var acc_a = 0.0;
    for (var n = 0; n < taps; n = n + 1) {
        dir = vec2f(
            dir.x * CA_GA_COS - dir.y * CA_GA_SIN,
            dir.x * CA_GA_SIN + dir.y * CA_GA_COS,
        );
        // sqrt radius → uniform disk density (sunflower distribution).
        let r = sqrt(f32(n) / f32(taps)) * blur_px;
        let s = aberration_sample_uv(tex, samp, uv + dir * r / dims);
        acc_rgb = acc_rgb + s.rgb * s.a;
        acc_a = acc_a + s.a;
    }
    return vec4f(acc_rgb / max(acc_a, CA_EPS), acc_a / f32(taps));
}

// The full effect at `uv`. The base image passes through untouched; each entry
// then *displaces its own hue's content* on top of it. For entry *i* the sample
// position is `center + (uv − center)·scale + offset` (offset px → UV via
// `dims`), and `d_p` is that displaced sample's premultiplied delta from the
// base. The delta splits two ways: `k1` shifts the whole pixel (the achromatic
// share — a white entry has k1=1, moving everything), and `k2·axis·dot(axis,d_p)`
// shifts only the component along the entry's hue axis (a primary entry has
// k2=1 and an exact channel axis, giving a classic channel split). Alpha tracks
// the *coverage* change of the displaced sample (`s.a − base.a`) — not the color
// delta — so a pure hue shift inside a solid region leaves opacity untouched,
// while content crossing an alpha edge still gains/loses coverage.
//
// Identity (offset 0, scale 1, blur 0) ⇒ d_p = 0 ⇒ bit-exact passthrough for any
// entry colors; a flat image is unchanged except at gradients/edges where the
// displaced sample differs — the physical CA signature.
fn aberration_apply(
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2f,
    params: AberrationParams,
) -> vec4f {
    let base = aberration_sample_uv(tex, samp, uv);
    if (params.count == 0u) {
        return base;
    }
    let dims = vec2f(textureDimensions(tex));
    let center = vec2f(0.5, 0.5);
    let b_p = base.rgb * base.a;     // premultiplied base
    var out_p = b_p;                 // premultiplied accumulator
    var acc_a = base.a;
    let n = min(params.count, CA_MAX);
    for (var i = 0u; i < n; i = i + 1u) {
        let ab = params.entries[i];
        let src_uv = center + (uv - center) * ab.scale + ab.offset_px / dims;
        let s = aberration_blurred(tex, samp, src_uv, ab.blur_px, dims);
        let d_p = s.rgb * s.a - b_p;            // premultiplied content delta
        let proj = dot(ab.axis, d_p);           // color reconstruction (color + coverage move)
        out_p = out_p + ab.k1 * d_p + ab.k2 * ab.axis * proj;
        // Alpha follows coverage change only: the displaced sample's alpha delta,
        // weighted (for the chromatic share) by how much of the base pixel's
        // content lies along the entry's hue axis. When the tap stays in a solid
        // region (`cov == 0`) a pure color shift leaves alpha untouched.
        let cov = s.a - base.a;
        acc_a = acc_a + ab.k1 * cov + ab.k2 * dot(ab.axis, base.rgb) * cov;
    }
    // Negative fringes (hue-opposite content) and multi-entry overshoot can push
    // a premultiplied channel below zero; clamp before un-premultiplying.
    out_p = max(out_p, vec3f(0.0));
    // Representability floor: a channel that survives while another departs off
    // an alpha edge must stay visible — alpha can't drop below the largest
    // premultiplied channel (opaque yellow losing its red stays opaque green).
    var out_a = clamp(acc_a, 0.0, 1.0);
    out_a = max(out_a, min(max(out_p.r, max(out_p.g, out_p.b)), 1.0));
    return vec4f(out_p / max(out_a, CA_EPS), out_a);
}
