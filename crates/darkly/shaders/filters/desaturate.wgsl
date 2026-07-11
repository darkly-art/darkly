// Desaturate — RGB → gray by one of six formulas, matching Krita's
// desaturate adjustment (`krita/plugins/color/colorspaceextensions/
// kis_desaturate_adjustment.cpp`), which follows Tanner Helland's grayscale
// algorithm survey (https://www.tannerhelland.com/3643/grayscale-image-algorithm-vb6/).
// The Rust side (`gpu/filters/desaturate.rs`) packs the mode selector into the
// uniform. Alpha passes through; no output clamp — the unorm render target
// clamps on store.

@group(0) @binding(0) var t_src: texture_2d<f32>;

struct Params {
    mode: u32, // 0 lightness, 1 BT.709, 2 BT.601, 3 average, 4 min, 5 max
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
    _pad4: u32,
    _pad5: u32,
    _pad6: u32,
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

fn desaturate_gray(rgb: vec3f) -> f32 {
    let lo = min(rgb.r, min(rgb.g, rgb.b));
    let hi = max(rgb.r, max(rgb.g, rgb.b));
    switch (params.mode) {
        case 0u: { return (hi + lo) / 2.0; } // lightness
        // The two luminosity modes use the standard BT.709 / BT.601 luma
        // coefficients — the same constants as `lib/colorspace.wgsl` (HCY_R/G/B)
        // and `composite.wgsl` (pd_lum); inlined rather than prepending the
        // colorspace lib for two constants. Intentional sharing, not drift.
        case 1u: { return dot(rgb, vec3f(0.2126, 0.7152, 0.0722)); } // BT.709
        case 2u: { return dot(rgb, vec3f(0.299, 0.587, 0.114)); } // BT.601
        case 3u: { return (rgb.r + rgb.g + rgb.b) / 3.0; } // average
        case 4u: { return lo; } // min
        default: { return hi; } // max
    }
}

fn desaturate_transform(rgb: vec3f) -> vec3f {
    return vec3f(desaturate_gray(rgb));
}

@fragment
fn fs_desaturate(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let c = textureLoad(t_src, p, 0);
    return vec4f(desaturate_transform(c.rgb), c.a);
}

// Destructive selection-clipped variant: transform where the R8 mask is
// selected (>0.5), pass the original through elsewhere (mirrors
// `fs_invert_masked`).
@group(0) @binding(2) var t_mask: texture_2d<f32>;

@fragment
fn fs_desaturate_masked(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let orig = textureLoad(t_src, p, 0);
    let filtered = vec4f(desaturate_transform(orig.rgb), orig.a);
    let selected = textureLoad(t_mask, p, 0).r > 0.5;
    return select(orig, filtered, selected);
}
