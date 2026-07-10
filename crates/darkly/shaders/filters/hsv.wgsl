// Hue / Saturation / Value adjustment — Krita's `hsvadjustment`
// (`plugins/color/colorspaceextensions/kis_hsv_adjustment.cpp`, `HSVTransform`
// + colorize). Four modes:
//
//   model 0 HSV  — hue rotate + saturation/value scale in HSV
//   model 1 HSL  — same, in HSL
//   model 2 HSY  — same, in luma-weighted HCY (Krita calls it "HSY")
//   colorize     — absolute hue/sat, luma preserved (like PS Hue/Saturation);
//                  overrides the model selector
//
// The colour-space conversions (`rgb_to_hsv`/`hsv_to_rgb`, `rgb_to_hsl`/…,
// `rgb_to_hsy`/`hsy_to_rgb`) are prepended from `shaders/lib/colorspace.wgsl`.
// The Rust side (`gpu/filters/hsv.rs`) packs the params: `hue` in degrees
// (−180..180), `saturation`/`value` normalized to −1..1, `model`/`colorize`
// as u32.

@group(0) @binding(0) var t_src: texture_2d<f32>;

struct Params {
    hue: f32,        // degrees, −180..180
    saturation: f32, // −1..1
    value: f32,      // −1..1
    model: u32,      // 0 = HSV, 1 = HSL, 2 = HSY
    colorize: u32,   // 0 / 1
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};
@group(0) @binding(1) var<uniform> params: Params;

struct VsOut { @builtin(position) pos: vec4f };

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle.
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn wrap360(h: f32) -> f32 {
    return h - floor(h / 360.0) * 360.0;
}

// Saturation scale — Krita `HSVTransform`: a nonlinear boost for ds > 0
// (ds=0.5 → ×2, ds=1 → ×4), a plain fade for ds ≤ 0 (ds=−1 → 0, fully grey).
fn apply_sat(s: f32, ds: f32) -> f32 {
    if (ds > 0.0) {
        return min(1.0, s * (1.0 + ds + 2.0 * ds * ds));
    }
    return s * (ds + 1.0);
}

// Value/lightness/luma scale — Krita `HSVTransform`: dv > 0 lerps toward 1,
// dv < 0 fades toward 0. dv=0 is identity.
fn apply_val(v: f32, dv: f32) -> f32 {
    if (dv > 0.0) {
        return v + dv * (1.0 - v);
    }
    return v * (dv + 1.0);
}

fn hsv_transform(rgb: vec3f) -> vec3f {
    let dh = params.hue;
    let ds = params.saturation;
    let dv = params.value;

    // No-op fast path keeps an identity adjustment bit-exact (no round-trip
    // drift) — the analog of the LUT filter's stage gates.
    if (params.colorize == 0u && dh == 0.0 && ds == 0.0 && dv == 0.0) {
        return rgb;
    }

    if (params.colorize != 0u) {
        // Absolute hue/sat, luma preserved. `hsy_to_rgb` caps chroma to gamut
        // so the original luma survives exactly (value shifts it deliberately).
        var y = HCY_R * rgb.r + HCY_G * rgb.g + HCY_B * rgb.b;
        if (dv > 0.0) {
            y = y * (1.0 - dv) + dv;
        } else if (dv < 0.0) {
            y = y * (dv + 1.0);
        }
        var h_abs = dh;
        if (h_abs < 0.0) { h_abs += 360.0; }
        let c_abs = clamp((ds + 1.0) * 0.5, 0.0, 1.0);
        return hsy_to_rgb(vec3f(h_abs / 360.0, c_abs, y));
    }

    if (params.model == 0u) {
        var hsv = rgb_to_hsv(rgb);
        hsv.x = wrap360(hsv.x + dh);
        hsv.y = apply_sat(hsv.y, ds);
        hsv.z = apply_val(hsv.z, dv);
        return hsv_to_rgb(hsv);
    } else if (params.model == 1u) {
        var hsl = rgb_to_hsl(rgb);
        hsl.x = wrap360(hsl.x + dh);
        hsl.y = apply_sat(hsl.y, ds);
        hsl.z = apply_val(hsl.z, dv);
        return hsl_to_rgb(hsl);
    } else {
        var hcy = rgb_to_hsy(rgb);
        hcy.x = fract(hcy.x + dh / 360.0);
        hcy.y = apply_sat(hcy.y, ds); // chroma
        hcy.z = apply_val(hcy.z, dv); // luma
        return hsy_to_rgb(hcy);
    }
}

@fragment
fn fs_hsv(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let c = textureLoad(t_src, p, 0);
    return vec4f(hsv_transform(c.rgb), c.a);
}

// Destructive selection-clipped variant: transform where the R8 mask is
// selected (>0.5), pass the original through elsewhere (mirrors
// `fs_invert_masked`).
@group(0) @binding(2) var t_mask: texture_2d<f32>;

@fragment
fn fs_hsv_masked(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let orig = textureLoad(t_src, p, 0);
    let filtered = vec4f(hsv_transform(orig.rgb), orig.a);
    let selected = textureLoad(t_mask, p, 0).r > 0.5;
    return select(orig, filtered, selected);
}
