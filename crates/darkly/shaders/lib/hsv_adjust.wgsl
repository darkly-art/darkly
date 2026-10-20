// Hue / saturation / value adjustment shared by the Hue/Saturation effect
// (`effects/hsv.wgsl`) and the brush graph's `hsv` node (every compiled
// brush shader prepends this lib). Requires `lib/colorspace.wgsl`
// (`rgb_to_hsv`, `hsv_to_rgb`) ahead of it. The CPU twin is
// `crates/darkly/src/color.rs`; the two must stay in numerical parity.
//
// The saturation and value curves are Krita's `HSVTransform`
// (`plugins/color/colorspaceextensions/kis_hsv_adjustment.cpp`, Cyrille
// Berger and Dmitry Kazakov).

fn wrap360(h: f32) -> f32 {
    return h - floor(h / 360.0) * 360.0;
}

// Saturation scale: a nonlinear boost for ds > 0 (ds=0.5 → ×2, ds=1 → ×4,
// capped at 1), a plain fade for ds ≤ 0 (ds=−1 → 0, fully grey).
fn apply_sat(s: f32, ds: f32) -> f32 {
    if (ds > 0.0) {
        return min(1.0, s * (1.0 + ds + 2.0 * ds * ds));
    }
    return s * (ds + 1.0);
}

// Value/lightness/luma scale: dv > 0 lerps toward 1, dv < 0 fades toward 0.
// dv=0 is identity.
fn apply_val(v: f32, dv: f32) -> f32 {
    if (dv > 0.0) {
        return v + dv * (1.0 - v);
    }
    return v * (dv + 1.0);
}

// Rotate hue by `dh` degrees and scale saturation / value by `ds` / `dv` in
// −1..1, in the HSV model. All-zero deltas return the input bit-exactly: no
// round trip through HSV, so an identity adjustment never drifts.
fn hsv_adjust(rgb: vec3f, dh: f32, ds: f32, dv: f32) -> vec3f {
    if (dh == 0.0 && ds == 0.0 && dv == 0.0) {
        return rgb;
    }
    var hsv = rgb_to_hsv(rgb);
    hsv.x = wrap360(hsv.x + dh);
    hsv.y = apply_sat(hsv.y, ds);
    hsv.z = apply_val(hsv.z, dv);
    return hsv_to_rgb(hsv);
}
