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
    show_mask: u32,
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

fn blend(fg: vec4f, bg: vec4f, mode: u32) -> vec4f {
    // Blend modes operate on straight-alpha colors (PDF/SVG spec).
    var Cs: vec3f;
    switch mode {
        case 0u: { Cs = fg.rgb; }                                    // Normal
        case 1u: { Cs = fg.rgb * bg.rgb; }                           // Multiply
        case 2u: { Cs = fg.rgb + bg.rgb - fg.rgb * bg.rgb; }         // Screen
        case 3u: {                                                    // Overlay
            let lo = 2.0 * fg.rgb * bg.rgb;
            let hi = 1.0 - 2.0 * (1.0 - fg.rgb) * (1.0 - bg.rgb);
            Cs = select(hi, lo, bg.rgb < vec3f(0.5));
        }
        default: { Cs = fg.rgb; }
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

    // Show mask as grayscale (GIMP's show_mask mode)
    if (uniforms.show_mask != 0u) {
        return vec4f(mask_alpha, mask_alpha, mask_alpha, 1.0);
    }

    var fg = select(vec4f(0.0), textureSample(t_layer, t_sampler, layer_uv), in_bounds);
    fg = vec4f(fg.rgb, fg.a * uniforms.opacity * mask_alpha);
    return blend(fg, bg, uniforms.blend_mode);
}
