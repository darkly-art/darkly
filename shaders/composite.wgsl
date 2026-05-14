// Tile compositing: sample background accumulator + layer, apply blend mode.
// Group 0: layer blend inputs. Group 1: layer mask texture.

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}

@group(0) @binding(0) var t_bg: texture_2d<f32>;
@group(0) @binding(1) var t_layer: texture_2d<f32>;
@group(0) @binding(2) var t_sampler: sampler;

struct Uniforms {
    opacity: f32,
    blend_mode: u32,
    isolated: u32,
    _pad1: f32,
    // Layer pixel offset in canvas coords (top-left).
    layer_offset: vec2f,
    // Layer texture dimensions in pixels.
    layer_size: vec2f,
    // Canvas dimensions in pixels.
    canvas_size: vec2f,
    _pad2: vec2f,
}
@group(0) @binding(3) var<uniform> uniforms: Uniforms;

// Mask texture in a separate bind group — avoids rebuilding group 0 on mask change.
// When no mask is present, a 1x1 white fallback texture is bound (mask_alpha=1.0).
@group(1) @binding(0) var t_mask: texture_2d<f32>;

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

fn blend(fg: vec4f, bg: vec4f, mode: u32) -> vec4f {
    // Blend modes operate on straight-alpha colors (PDF/SVG spec).
    //
    // The `case` arms are generated at runtime from the blend-mode registry —
    // each `crates/darkly/src/gpu/blend_modes/<name>.rs` declares its own WGSL
    // math, and `gpu::blend_mode::build_composite_source` splices them in
    // before this shader is compiled. Edit a blend mode's `.rs` file, not the
    // switch table.
    var Cs: vec3f;
    switch mode {
        // @blend-switch
    }

    // Porter-Duff source-over compositing (PDF 11.3.7):
    // Cr = (αs · lerp(Cs_src, B(Cs,Cb), αb) + (1−αs)·αb·Cb) / αo
    let out_a = fg.a + bg.a * (1.0 - fg.a);
    let out_rgb = (fg.a * mix(fg.rgb, Cs, bg.a) + (1.0 - fg.a) * bg.a * bg.rgb)
               / max(out_a, 0.001);
    return vec4f(out_rgb, out_a);
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let bg = textureSample(t_bg, t_sampler, in.uv);

    // Translate canvas UV → layer UV via the layer's offset+size in canvas
    // coords. When the layer's bounds match the canvas (the default), this
    // collapses to layer_uv == in.uv. The mask shares the layer's bounds,
    // so the same UV samples both textures.
    let canvas_pos = in.uv * uniforms.canvas_size;
    let layer_pos = canvas_pos - uniforms.layer_offset;
    let layer_uv = layer_pos / uniforms.layer_size;
    let in_bounds = all(layer_uv >= vec2f(0.0)) && all(layer_uv <= vec2f(1.0));

    // textureSample requires uniform control flow, so sample unconditionally
    // and clamp the result outside the layer bounds. Outside the mask's
    // bounds we treat coverage as 0 (no contribution), matching the layer.
    let mask_raw = textureSample(t_mask, t_sampler, layer_uv).r;
    let mask_alpha = select(0.0, mask_raw, in_bounds);

    // Show mask as grayscale (GIMP's isolated mode)
    if (uniforms.isolated != 0u) {
        return vec4f(mask_alpha, mask_alpha, mask_alpha, 1.0);
    }

    var fg = select(vec4f(0.0), textureSample(t_layer, t_sampler, layer_uv), in_bounds);
    fg = vec4f(fg.rgb, fg.a * uniforms.opacity * mask_alpha);
    return blend(fg, bg, uniforms.blend_mode);
}
