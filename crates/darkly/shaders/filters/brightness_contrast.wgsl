// Brightness/Contrast adjustment: GIMP's mapping (Michael Natterer,
// `gimp/app/operations/gimpoperationbrightnesscontrast.c`,
// https://gitlab.gnome.org/GNOME/gimp/-/blob/master/app/operations/gimpoperationbrightnesscontrast.c):
// brightness lerps toward black/white, contrast slants the curve through
// mid-gray. The Rust side (`gpu/filters/brightness_contrast.rs`) packs the
// shader-ready values: `brightness` already halved to −0.5..0.5, `slant`
// precomputed as tan((c+1)·π/4) with contrast 0 pinned to exactly 1.0.
// Applied per RGB channel; alpha passes through. No explicit output clamp;
// the unorm render target clamps on store.

@group(0) @binding(0) var t_src: texture_2d<f32>;

struct Params {
    brightness: f32, // −0.5..0.5 (slider halved, per GIMP)
    slant: f32,      // tan((contrast+1)·π/4); exactly 1.0 at contrast 0
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
    _pad4: u32,
    _pad5: u32,
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

fn bc_map(v: f32) -> f32 {
    var value = v;
    if (params.brightness < 0.0) {
        value = value * (1.0 + params.brightness);
    } else {
        value = value + (1.0 - value) * params.brightness;
    }
    return (value - 0.5) * params.slant + 0.5;
}

fn bc_transform(rgb: vec3f) -> vec3f {
    // No-op fast path keeps an identity adjustment bit-exact, sound because
    // the Rust packing pins slant to exactly 1.0 at contrast 0.
    if (params.brightness == 0.0 && params.slant == 1.0) {
        return rgb;
    }
    return vec3f(bc_map(rgb.r), bc_map(rgb.g), bc_map(rgb.b));
}

@fragment
fn fs_bc(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let c = textureLoad(t_src, p, 0);
    return vec4f(bc_transform(c.rgb), c.a);
}

// Destructive selection-clipped variant: transform where the R8 mask is
// selected (>0.5), pass the original through elsewhere (mirrors
// `fs_invert_masked`).
@group(0) @binding(2) var t_mask: texture_2d<f32>;

@fragment
fn fs_bc_masked(in: VsOut) -> @location(0) vec4f {
    let p = vec2i(floor(in.pos.xy));
    let orig = textureLoad(t_src, p, 0);
    let filtered = vec4f(bc_transform(orig.rgb), orig.a);
    let selected = textureLoad(t_mask, p, 0).r > 0.5;
    return select(orig, filtered, selected);
}
