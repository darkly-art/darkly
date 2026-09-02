// Alpha-weighted magnification, used to bring a reduced-resolution effect's
// output back up to its destination size.
//
// A plain bilinear blit would be wrong here for the same reason the unweighted
// downscale was: canvas accumulators hold *straight* (non-premultiplied) alpha,
// since composite.wgsl divides its result by `out_a`. Fully transparent texels
// therefore carry RGB 0, and hardware bilinear filtering mixes that black into
// every colour it interpolates across an alpha edge — darkening a band one
// output texel wide around every silhouette in the document, which then ships
// into the export.
//
// So the bilerp is done by hand from four `textureLoad`s, with colour weighted
// by coverage and alpha interpolated on its own. Same pass, same bind group,
// no extra textures. The weighting idiom matches lib/aberration.wgsl.

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

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

@fragment fn fs_upscale(in: VertexOutput) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(t_input));
    let hi = vec2i(dims) - vec2i(1, 1);

    // Texel-center convention: the sample position sits half a texel in.
    let p = in.uv * dims - vec2f(0.5, 0.5);
    let base = floor(p);
    let f = p - base;
    let b = vec2i(base);

    let s00 = textureLoad(t_input, clamp(b + vec2i(0, 0), vec2i(0), hi), 0);
    let s10 = textureLoad(t_input, clamp(b + vec2i(1, 0), vec2i(0), hi), 0);
    let s01 = textureLoad(t_input, clamp(b + vec2i(0, 1), vec2i(0), hi), 0);
    let s11 = textureLoad(t_input, clamp(b + vec2i(1, 1), vec2i(0), hi), 0);

    let w00 = (1.0 - f.x) * (1.0 - f.y);
    let w10 = f.x * (1.0 - f.y);
    let w01 = (1.0 - f.x) * f.y;
    let w11 = f.x * f.y;

    // Colour carried by coverage; alpha is an ordinary bilerp.
    let acc_rgb = s00.rgb * (s00.a * w00)
                + s10.rgb * (s10.a * w10)
                + s01.rgb * (s01.a * w01)
                + s11.rgb * (s11.a * w11);
    let acc_a = s00.a * w00 + s10.a * w10 + s01.a * w01 + s11.a * w11;

    return vec4f(acc_rgb / max(acc_a, 1.0e-4), acc_a);
}
