// Per-channel tone-curve adjustment: Krita's "Color Adjustment Curves" model
// (plugins/filters/colorsfilters, KisMultiChannelFilter). Channels, in order:
//
//   RGB (composite), Red, Green, Blue, Alpha, Hue, Saturation, Lightness
//
// The curves are baked on the CPU (`gpu/filters/curves.rs`) into a 256×2 RGBA8
// LUT read at integer index `round(c*255)` (no sampler):
//   row 0 (.rgba) - per-component color curves. `.rgb[i] = rgb(channel(i))`
//                   (the per-channel curve first, then the composite "RGB"
//                   curve on top, per Krita's transform order); `.a[i] = alpha(i)`,
//                   with the composite curve NOT applied to alpha.
//   row 1 (.rgb)  - `.r = hue(i)`, `.g = saturation(i)`, `.b = lightness(i)`.
//
// The color-component stage is bit-exact for identity curves (LUT[i]==i), so it
// always runs. The Hue/Saturation (HSV) and Lightness (CIELAB L*) stages need a
// color-space round trip that is only ~identity in float, so each is gated by a
// `_active` flag (set when its curve is non-identity), exactly as Krita skips
// null transforms, keeping an all-identity Curves layer a byte-for-byte no-op.

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

// The HSV and CIELAB conversions (`rgb_to_hsv`/`hsv_to_rgb`, `rgb_to_lab`/
// `lab_to_rgb`) are prepended from `shaders/lib/colorspace.wgsl` at load time.

// Apply the baked LUT + gated HSV/Lab stages to one source texel. Shared by the
// plain and masked entry points.
fn curves_transform(c_in: vec4f) -> vec4f {
    var c = c_in;

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

@fragment
fn fs_curves(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    return curves_transform(textureLoad(t_src, p, 0));
}

// Destructive selection-clipped variant: filter where the R8 mask is selected
// (>0.5), pass the original through elsewhere (mirrors `fs_invert_masked`).
@group(0) @binding(3) var t_mask: texture_2d<f32>;

@fragment
fn fs_curves_masked(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let orig = textureLoad(t_src, p, 0);
    let filtered = curves_transform(orig);
    let selected = textureLoad(t_mask, p, 0).r > 0.5;
    return select(orig, filtered, selected);
}
