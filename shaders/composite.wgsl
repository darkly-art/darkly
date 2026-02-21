// Tile compositing: sample background accumulator + layer, apply blend mode.

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
    _pad0: f32,
    _pad1: f32,
}
@group(0) @binding(3) var<uniform> uniforms: Uniforms;

fn blend(fg: vec4f, bg: vec4f, mode: u32) -> vec4f {
    let fg_pre = fg.rgb * fg.a;
    let bg_pre = bg.rgb * bg.a;
    var blended_rgb: vec3f;

    switch mode {
        // Normal
        case 0u: {
            blended_rgb = fg_pre;
        }
        // Multiply
        case 1u: {
            blended_rgb = fg_pre * bg_pre;
        }
        // Screen
        case 2u: {
            blended_rgb = fg_pre + bg_pre - fg_pre * bg_pre;
        }
        // Overlay
        case 3u: {
            let lo = 2.0 * fg_pre * bg_pre;
            let hi = 1.0 - 2.0 * (1.0 - fg_pre) * (1.0 - bg_pre);
            blended_rgb = select(hi, lo, bg_pre < vec3f(0.5));
        }
        default: {
            blended_rgb = fg_pre;
        }
    }

    let out_a = fg.a + bg.a * (1.0 - fg.a);
    let out_rgb = mix(bg_pre, blended_rgb, fg.a) / max(out_a, 0.001);
    return vec4f(out_rgb, out_a);
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let bg = textureSample(t_bg, t_sampler, in.uv);
    var fg = textureSample(t_layer, t_sampler, in.uv);
    fg = vec4f(fg.rgb, fg.a * uniforms.opacity);
    return blend(fg, bg, uniforms.blend_mode);
}
