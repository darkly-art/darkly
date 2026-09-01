// The blend-mode math every in-place and layer composite shares.
//
// Two shaders need it: `composite.wgsl`, which blends a layer over the
// accumulator, and `in_place_apply.wgsl`, which blends a transform's result
// over the image it transformed. Neither owns it, so it lives here and both
// get it prepended by `gpu::blend_mode::build_blend_source`.
//
// The `case` arms of `blend_rgb` are generated at runtime from the blend-mode
// registry — each `crates/darkly/src/gpu/blend_modes/<name>.rs` declares its
// own WGSL math, and `gpu::blend_mode::build_blend_source` splices them into
// the marker below before compilation. Edit a blend mode's `.rs` file, not
// this switch.

// Color Burn — Krita KoCompositeOpFunctions.h:329–361.
// d=1 is a stable point; s=0 forces full burn. NaN/Inf are masked rather
// than relying on IEEE behavior (WGSL doesn't guarantee it across backends).
fn pd_color_burn(s: vec3f, d: vec3f) -> vec3f {
    let safe_s = max(s, vec3f(1e-7));
    let raw = vec3f(1.0) - (vec3f(1.0) - d) / safe_s;
    var out = clamp(raw, vec3f(0.0), vec3f(1.0));
    out = select(out, vec3f(0.0), s <= vec3f(0.0));
    out = select(out, vec3f(1.0), d >= vec3f(1.0));
    return out;
}

// Color Dodge — Krita KoCompositeOpFunctions.h:376–403.
// s=1 lights up only where the destination has signal.
fn pd_color_dodge(s: vec3f, d: vec3f) -> vec3f {
    let safe_denom = max(vec3f(1.0) - s, vec3f(1e-7));
    let raw = d / safe_denom;
    let one_or_zero = select(vec3f(0.0), vec3f(1.0), d > vec3f(0.0));
    var out = clamp(raw, vec3f(0.0), vec3f(1.0));
    out = select(out, one_or_zero, s >= vec3f(1.0));
    return out;
}

// Soft Light — Photoshop variant (Krita KoCompositeOpFunctions.h:513–529).
fn pd_soft_light(s: vec3f, d: vec3f) -> vec3f {
    let lighten = d + (2.0 * s - vec3f(1.0)) * (sqrt(d) - d);
    let darken = d - (vec3f(1.0) - 2.0 * s) * d * (vec3f(1.0) - d);
    return select(darken, lighten, s > vec3f(0.5));
}

// HSL helpers — PDF 11.3.5.3 / W3C Compositing-1, matching Krita's HSY model
// (luma weights from KoColorSpaceMaths.h:912).
fn pd_lum(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
}

fn pd_clip_color(c: vec3f) -> vec3f {
    let l = pd_lum(c);
    let n = min(min(c.r, c.g), c.b);
    let x = max(max(c.r, c.g), c.b);
    var out = c;
    // Conditions test the original n/x; each branch's update reads the running
    // `out`, so a triggered low-clip feeds into a subsequent high-clip
    // (matching Krita's ToneMapping in KoColorSpaceMaths.h:1052).
    if (n < 0.0) {
        out = vec3f(l) + ((out - vec3f(l)) * l) / (l - n);
    }
    if (x > 1.0) {
        out = vec3f(l) + ((out - vec3f(l)) * (1.0 - l)) / (x - l);
    }
    return out;
}

fn pd_set_lum(c: vec3f, l: f32) -> vec3f {
    return pd_clip_color(c + vec3f(l - pd_lum(c)));
}

fn pd_sat(c: vec3f) -> f32 {
    return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
}

fn pd_set_sat(c: vec3f, s: f32) -> vec3f {
    let cmax = max(max(c.r, c.g), c.b);
    let cmin = min(min(c.r, c.g), c.b);
    let range = cmax - cmin;
    if (range <= 0.0) {
        return vec3f(0.0);
    }
    return (c - vec3f(cmin)) * (s / range);
}

/// The blend mode's own math: what colour a source makes over a backdrop,
/// before any compositing. Straight-alpha, per the PDF/SVG spec.
///
/// Separated from `blend` because the two consumers want different amounts of
/// it. A layer composite needs this *and* Porter-Duff source-over; an in-place
/// apply needs only this, since source-over of a transform's result over its
/// own input would inflate alpha at partially transparent texels.
fn blend_rgb(fg: vec4f, bg: vec4f, mode: u32) -> vec3f {
    var Cs: vec3f;
    switch mode {
        // @blend-switch
    }
    return Cs;
}
