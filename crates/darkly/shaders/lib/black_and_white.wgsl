// Black and White — shared RGB → gray transform used by both surfaces of the
// `black_and_white` type: the veil (`shaders/veils/black_and_white.wgsl`) and
// the filter (`shaders/filters/black_and_white.wgsl`). Each wrapper declares
// its own `var<uniform> params: BwParams` at its own binding slot; this file
// carries only binding-free declarations, so it is prepended verbatim.
//
// The six fixed formulas match Krita's desaturate adjustment
// (`krita/plugins/color/colorspaceextensions/kis_desaturate_adjustment.cpp`),
// which follows Tanner Helland's grayscale algorithm survey
// (https://www.tannerhelland.com/3643/grayscale-image-algorithm-vb6/).
// Mode 6 is a custom weighted mix. The Rust side (`gpu/black_and_white.rs`)
// normalizes the weights and converts the tint hue to RGB when packing the
// uniform, so no per-pixel normalization or HSV math happens here.

struct BwParams {
    // 0 lightness, 1 BT.709, 2 BT.601, 3 average, 4 min, 5 max,
    // 6 custom weights
    mode: u32,
    // Custom-weight coefficients, pre-normalized to sum 1.
    red_weight: f32,
    green_weight: f32,
    blue_weight: f32,
    // Tint color precomputed from the hue param (s = v = 1).
    tint_r: f32,
    tint_g: f32,
    tint_b: f32,
    tint_strength: f32,
}

fn bw_gray(rgb: vec3f, p: BwParams) -> f32 {
    let lo = min(rgb.r, min(rgb.g, rgb.b));
    let hi = max(rgb.r, max(rgb.g, rgb.b));
    switch (p.mode) {
        case 0u: { return (hi + lo) / 2.0; } // lightness
        // The two luminosity modes use the standard BT.709 / BT.601 luma
        // coefficients — the same constants as `lib/colorspace.wgsl` (HCY_R/G/B)
        // and `composite.wgsl` (pd_lum); inlined rather than prepending the
        // colorspace lib for two constants. Intentional sharing, not drift.
        case 1u: { return dot(rgb, vec3f(0.2126, 0.7152, 0.0722)); } // BT.709
        case 2u: { return dot(rgb, vec3f(0.299, 0.587, 0.114)); } // BT.601
        case 3u: { return (rgb.r + rgb.g + rgb.b) / 3.0; } // average
        case 4u: { return lo; } // min
        case 5u: { return hi; } // max
        default: {
            return dot(rgb, vec3f(p.red_weight, p.green_weight, p.blue_weight));
        }
    }
}

// Gray the color by the selected formula, then mix toward the tinted gray.
fn bw_transform(rgb: vec3f, p: BwParams) -> vec3f {
    let gray = bw_gray(rgb, p);
    let tinted = gray * vec3f(p.tint_r, p.tint_g, p.tint_b);
    return mix(vec3f(gray), tinted, p.tint_strength);
}
