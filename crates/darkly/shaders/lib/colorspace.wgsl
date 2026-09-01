// Shared colour-space conversions: HSV (Krita KoColorConversions RGBToHSV /
// HSVToRGB) and CIELAB (sRGB / D65). Prepended to both the LUT filter shader
// (`filters/curves.wgsl`, used by Curves and Levels) and `histogram.wgsl`, so
// the histogram bins Hue/Saturation/Lightness with the exact math the filter
// applies. Kept in one file rather than duplicated across the two shaders.

const EPS: f32 = 1e-6;

// --- HSV ---------------------------------------------------------------------

// Returns h in [0,360) (0 when achromatic), s and v in [0,1].
fn rgb_to_hsv(c: vec3f) -> vec3f {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let v = mx;
    var s = 0.0;
    if (mx > EPS) { s = (mx - mn) / mx; }
    var h = 0.0;
    if (s >= EPS) {
        let delta = mx - mn;
        if (c.r == mx) {
            h = (c.g - c.b) / delta;
        } else if (c.g == mx) {
            h = 2.0 + (c.b - c.r) / delta;
        } else {
            h = 4.0 + (c.r - c.g) / delta;
        }
        h *= 60.0;
        if (h < 0.0) { h += 360.0; }
    }
    return vec3f(h, s, v);
}

fn hsv_to_rgb(hsv: vec3f) -> vec3f {
    let s = hsv.y;
    let v = hsv.z;
    if (s < EPS) { return vec3f(v, v, v); }
    var h = hsv.x;
    if (h > 360.0 - EPS) { h -= 360.0; }
    h /= 60.0;
    let i = i32(floor(h));
    let f = h - f32(i);
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    switch (i) {
        case 0: { return vec3f(v, t, p); }
        case 1: { return vec3f(q, v, p); }
        case 2: { return vec3f(p, v, t); }
        case 3: { return vec3f(p, q, v); }
        case 4: { return vec3f(t, p, v); }
        default: { return vec3f(v, p, q); }
    }
}

// --- HSL (Krita KoColorConversions RGBToHSL / HSLToRGB, "A Fast HSL-to-RGB
//     Transform" by Ken Fishkin, Graphics Gems 1990) --------------------------

// h in [0,360), s and l in [0,1]. Achromatic hue collapses to 0.
fn rgb_to_hsl(c: vec3f) -> vec3f {
    let v = max(c.r, max(c.g, c.b));
    let m = min(c.r, min(c.g, c.b));
    let l = (m + v) * 0.5;
    if (l <= 0.0) { return vec3f(0.0, 0.0, 0.0); }
    let vm = v - m;
    if (vm <= 0.0) { return vec3f(0.0, 0.0, l); }
    var s = vm;
    if (l <= 0.5) { s /= (v + m); } else { s /= (2.0 - v - m); }
    let r2 = (v - c.r) / vm;
    let g2 = (v - c.g) / vm;
    let b2 = (v - c.b) / vm;
    var h: f32;
    if (c.r == v) {
        h = select(1.0 - g2, 5.0 + b2, c.g == m);
    } else if (c.g == v) {
        h = select(3.0 - b2, 1.0 + r2, c.b == m);
    } else {
        h = select(5.0 - r2, 3.0 + g2, c.r == m);
    }
    h *= 60.0;
    h = h - floor(h / 360.0) * 360.0; // fmod into [0,360)
    return vec3f(h, s, l);
}

fn hsl_to_rgb(hsl: vec3f) -> vec3f {
    let sl = hsl.y;
    let l = hsl.z;
    let v = select(l + sl - l * sl, l * (1.0 + sl), l <= 0.5);
    if (v <= 0.0) { return vec3f(0.0); }
    let m = l + l - v;
    let sv = (v - m) / v;
    var h = hsl.x - floor(hsl.x / 360.0) * 360.0;
    h /= 60.0;
    let sextant = i32(floor(h));
    let fr = h - f32(sextant);
    let vsf = v * sv * fr;
    let mid1 = m + vsf;
    let mid2 = v - vsf;
    switch (sextant) {
        case 0: { return vec3f(v, mid1, m); }
        case 1: { return vec3f(mid2, v, m); }
        case 2: { return vec3f(m, v, mid1); }
        case 3: { return vec3f(m, mid2, v); }
        case 4: { return vec3f(mid1, m, v); }
        default: { return vec3f(v, m, mid2); }
    }
}

// --- HSY = HCY (Krita KoColorConversions RGBToHCY / HCYToRGB). "HSY" is Krita's
//     name for luma-weighted HCY. Rec.601 luma weights (the same used by Krita's
//     desaturate and colour maths). Hue is carried in [0,1) here. -------------

const HCY_R: f32 = 0.299;
const HCY_G: f32 = 0.587;
const HCY_B: f32 = 0.114;

// Returns h in [0,1), chroma in [0,1], luma y in [0,1].
fn rgb_to_hsy(col: vec3f) -> vec3f {
    let minval = min(col.r, min(col.g, col.b));
    let maxval = max(col.r, max(col.g, col.b));
    let luma = HCY_R * col.r + HCY_G * col.g + HCY_B * col.b;
    let chroma = maxval - minval;
    var hue = 0.0;
    if (chroma > 0.0) {
        if (maxval == col.r) {
            hue = select((col.g - col.b) / chroma + 6.0, (col.g - col.b) / chroma, minval == col.b);
        } else if (maxval == col.g) {
            hue = (col.b - col.r) / chroma + 2.0;
        } else {
            hue = (col.r - col.g) / chroma + 4.0;
        }
        hue /= 6.0;
    }
    return vec3f(clamp(hue, 0.0, 1.0), max(chroma, 0.0), max(luma, 0.0));
}

// HCY → RGB, but with the requested chroma capped to the largest value that
// keeps the luma-offset RGB inside [0,1], so luma `y` is preserved *exactly*.
// This deviates from Krita's HCYToRGB, which clamps out-of-gamut RGB to ≥0
// post-hoc and thereby shifts luma; capping chroma instead is the defining
// luma-preserving property of the HSY model (and of colorize).
fn hsy_to_rgb(hcy: vec3f) -> vec3f {
    let hue = fract(hcy.x); // wrap into [0,1)
    let luma = clamp(hcy.z, 0.0, 1.0);
    let h6 = hue * 6.0;
    let f = 1.0 - abs((h6 - 2.0 * floor(h6 * 0.5)) - 1.0); // fmod(h6,2)
    let sextant = i32(h6);
    // Unit base: RGB at chroma = 1, before the luma offset.
    var base = vec3f(0.0);
    switch (sextant) {
        case 0: { base = vec3f(1.0, f, 0.0); }
        case 1: { base = vec3f(f, 1.0, 0.0); }
        case 2: { base = vec3f(0.0, 1.0, f); }
        case 3: { base = vec3f(0.0, f, 1.0); }
        case 4: { base = vec3f(f, 0.0, 1.0); }
        default: { base = vec3f(1.0, 0.0, f); }
    }
    let k = HCY_R * base.r + HCY_G * base.g + HCY_B * base.b;
    let d = base - vec3f(k); // per-channel slope in chroma
    // channel = d*c + luma; require 0 ≤ channel ≤ 1 on every channel.
    var cmax = max(hcy.y, 0.0);
    if (d.r > EPS) { cmax = min(cmax, (1.0 - luma) / d.r); } else if (d.r < -EPS) { cmax = min(cmax, -luma / d.r); }
    if (d.g > EPS) { cmax = min(cmax, (1.0 - luma) / d.g); } else if (d.g < -EPS) { cmax = min(cmax, -luma / d.g); }
    if (d.b > EPS) { cmax = min(cmax, (1.0 - luma) / d.b); } else if (d.b < -EPS) { cmax = min(cmax, -luma / d.b); }
    let c = max(0.0, cmax);
    return clamp(d * c + vec3f(luma), vec3f(0.0), vec3f(1.0));
}

// --- CIELAB (sRGB / D65, Krita's "Lightness L*a*b*" channel) ------------------

fn srgb_to_linear(c: f32) -> f32 {
    if (c <= 0.04045) { return c / 12.92; }
    return pow((c + 0.055) / 1.055, 2.4);
}
fn linear_to_srgb(c: f32) -> f32 {
    if (c <= 0.0031308) { return c * 12.92; }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

const XN: f32 = 0.95047;
const YN: f32 = 1.0;
const ZN: f32 = 1.08883;

fn lab_f(t: f32) -> f32 {
    if (t > 0.008856) { return pow(t, 1.0 / 3.0); }
    return 7.787 * t + 16.0 / 116.0;
}
fn lab_finv(f: f32) -> f32 {
    let f3 = f * f * f;
    if (f3 > 0.008856) { return f3; }
    return (f - 16.0 / 116.0) / 7.787;
}

// Returns L in [0,100], a and b in their usual CIELAB ranges.
fn rgb_to_lab(c: vec3f) -> vec3f {
    let r = srgb_to_linear(c.r);
    let g = srgb_to_linear(c.g);
    let b = srgb_to_linear(c.b);
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / XN;
    let y = (0.2126729 * r + 0.7151522 * g + 0.0721750 * b) / YN;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / ZN;
    let fx = lab_f(x);
    let fy = lab_f(y);
    let fz = lab_f(z);
    return vec3f(116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz));
}

fn lab_to_rgb(lab: vec3f) -> vec3f {
    let fy = (lab.x + 16.0) / 116.0;
    let fx = fy + lab.y / 500.0;
    let fz = fy - lab.z / 200.0;
    let x = XN * lab_finv(fx);
    let y = YN * lab_finv(fy);
    let z = ZN * lab_finv(fz);
    let r = 3.2404542 * x - 1.5371385 * y - 0.4985314 * z;
    let g = -0.9692660 * x + 1.8760108 * y + 0.0415560 * z;
    let b = 0.0556434 * x - 0.2040259 * y + 1.0572252 * z;
    return vec3f(
        clamp(linear_to_srgb(r), 0.0, 1.0),
        clamp(linear_to_srgb(g), 0.0, 1.0),
        clamp(linear_to_srgb(b), 0.0, 1.0),
    );
}
