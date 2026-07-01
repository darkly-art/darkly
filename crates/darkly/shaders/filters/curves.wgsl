// Per-channel tone-curve adjustment — Krita's "Color Adjustment Curves" model
// (plugins/filters/colorsfilters, KisMultiChannelFilter). Channels, in order:
//
//   RGB (composite), Red, Green, Blue, Alpha, Hue, Saturation, Lightness
//
// The curves are baked on the CPU (`gpu/filters/curves.rs`) into a 256×2 RGBA8
// LUT read at integer index `round(c*255)` (no sampler):
//   row 0 (.rgba) — per-component color curves. `.rgb[i] = rgb(channel(i))`
//                   (the per-channel curve first, then the composite "RGB"
//                   curve on top, per Krita's transform order); `.a[i] = alpha(i)`,
//                   with the composite curve NOT applied to alpha.
//   row 1 (.rgb)  — `.r = hue(i)`, `.g = saturation(i)`, `.b = lightness(i)`.
//
// The color-component stage is bit-exact for identity curves (LUT[i]==i), so it
// always runs. The Hue/Saturation (HSV) and Lightness (CIELAB L*) stages need a
// color-space round trip that is only ~identity in float, so each is gated by a
// `_active` flag (set when its curve is non-identity) — exactly as Krita skips
// null transforms — keeping an all-identity Curves layer a byte-for-byte no-op.

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var t_lut: texture_2d<f32>;

struct Flags {
    hsv_active: u32,
    lightness_active: u32,
    _pad0: u32,
    _pad1: u32,
};
@group(0) @binding(2) var<uniform> flags: Flags;

struct VsOut { @builtin(position) pos: vec4f };

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle.
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn lut_index(c: f32) -> i32 {
    return i32(round(clamp(c, 0.0, 1.0) * 255.0));
}

// --- HSV (matches Krita KoColorConversions RGBToHSV / HSVToRGB) --------------

const EPS: f32 = 1e-6;

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

// --- CIELAB (sRGB / D65) — Krita's "Lightness L*a*b*" channel ----------------

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

@fragment
fn fs_curves(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    var c = textureLoad(t_src, p, 0);

    // Per-component color curves (composite∘channel baked in row 0).
    c.r = textureLoad(t_lut, vec2i(lut_index(c.r), 0), 0).r;
    c.g = textureLoad(t_lut, vec2i(lut_index(c.g), 0), 0).g;
    c.b = textureLoad(t_lut, vec2i(lut_index(c.b), 0), 0).b;
    c.a = textureLoad(t_lut, vec2i(lut_index(c.a), 0), 0).a;

    // Hue + Saturation, in HSV. Non-relative (Krita perchannel): the curve
    // replaces the channel value. Both curves ride one HSV round trip.
    if (flags.hsv_active != 0u) {
        var hsv = rgb_to_hsv(c.rgb);
        hsv.x = textureLoad(t_lut, vec2i(lut_index(hsv.x / 360.0), 1), 0).r * 360.0;
        hsv.y = textureLoad(t_lut, vec2i(lut_index(hsv.y), 1), 0).g;
        c = vec4f(hsv_to_rgb(hsv), c.a);
    }

    // Lightness, on CIELAB L* (normalized to [0,1] for the LUT).
    if (flags.lightness_active != 0u) {
        var lab = rgb_to_lab(c.rgb);
        lab.x = textureLoad(t_lut, vec2i(lut_index(lab.x / 100.0), 1), 0).b * 100.0;
        c = vec4f(lab_to_rgb(lab), c.a);
    }

    return c;
}
