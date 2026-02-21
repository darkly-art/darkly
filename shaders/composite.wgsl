// Compositing fragment shader: blends a layer onto the accumulator.

@group(0) @binding(0) var t_bg: texture_2d<f32>;
@group(0) @binding(1) var t_layer: texture_2d<f32>;
@group(0) @binding(2) var t_sampler: sampler;

struct Uniforms {
    opacity: f32,
    blend_mode: u32,
    _pad0: u32,
    _pad1: u32,
}
@group(0) @binding(3) var<uniform> uniforms: Uniforms;

fn blend(fg: vec4f, bg: vec4f, mode: u32) -> vec4f {
    let fg_pre = fg.rgb * fg.a;
    let bg_pre = bg.rgb * bg.a;
    var out_rgb: vec3f;
    switch mode {
        case 0u: { out_rgb = fg_pre; }                                    // Normal
        case 1u: { out_rgb = fg_pre * bg_pre; }                           // Multiply
        case 2u: { out_rgb = fg_pre + bg_pre - fg_pre * bg_pre; }         // Screen
        case 3u: {                                                         // Overlay
            let threshold = vec3f(0.5);
            let lo = 2.0 * fg_pre * bg_pre;
            let hi = vec3f(1.0) - 2.0 * (vec3f(1.0) - fg_pre) * (vec3f(1.0) - bg_pre);
            out_rgb = select(hi, lo, bg_pre <= threshold);
        }
        default: { out_rgb = fg_pre; }
    }
    let out_a = fg.a + bg.a * (1.0 - fg.a);
    return vec4f(mix(bg_pre, out_rgb, fg.a) / max(out_a, 0.001), out_a);
}

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let bg = textureSample(t_bg, t_sampler, in.uv);
    var fg = textureSample(t_layer, t_sampler, in.uv);
    fg = vec4f(fg.rgb, fg.a * uniforms.opacity);
    return blend(fg, bg, uniforms.blend_mode);
}
