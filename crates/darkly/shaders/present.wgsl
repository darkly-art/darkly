// Final blit from accumulator to surface with view transform.

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

struct ViewTransform {
    row0: vec4f,
    row1: vec4f,
    row2: vec4f,
    bg: vec4f,
    // flags.x = mip level to read (integer-valued: floor of log2 of canvas
    //           texels per screen pixel, 0 when not minifying)
    // flags.y = 1 to snap the sample to a texel centre, 0 to filter
    flags: vec4f,
}

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> view: ViewTransform;

@fragment fn fs_present(in: VertexOutput) -> @location(0) vec4f {
    // Transform screen pixel -> window-local canvas pixel using the inverse view matrix
    let screen_pos = in.position.xy;
    let canvas_x = view.row0.x * screen_pos.x + view.row1.x * screen_pos.y + view.row2.x;
    let canvas_y = view.row0.y * screen_pos.x + view.row1.y * screen_pos.y + view.row2.y;

    // Sample using the padded texture size so texels map 1:1 to window-local canvas pixels.
    let tex_dims = vec2f(textureDimensions(t_source));
    let uv = vec2f(canvas_x, canvas_y) / tex_dims;
    let clamped_uv = clamp(uv, vec2f(0.0), vec2f(1.0));

    // The sampling policy is decided on the CPU (`ViewTransform::stamp_sampling`)
    // and arrives as two numbers. Snapping the UV to a level-0 texel centre
    // makes the linear sampler return that texel unfiltered; the explicit
    // level selects the box-reduced mip the minified footprint needs.
    let lod = view.flags.x;
    let use_nearest = view.flags.y > 0.5;
    let snapped_uv = (floor(clamped_uv * tex_dims) + vec2f(0.5)) / tex_dims;
    let sample_uv = select(clamped_uv, snapped_uv, use_nearest);
    let color = textureSampleLevel(t_source, t_sampler, sample_uv, lod);

    // OOB check uses actual canvas dimensions (unpadded) so the tile
    // padding area shows as workspace background, not black.
    let canvas_dims = vec2f(view.row0.z, view.row1.z);
    let oob = canvas_x < 0.0 || canvas_x > canvas_dims.x
           || canvas_y < 0.0 || canvas_y > canvas_dims.y;

    // Composite the canvas over a screen-space checker so any transparency
    // in the final composite reads as transparency, not as darkened-by-
    // discarded-alpha. The composite cache is straight-alpha (composite.wgsl's
    // Porter-Duff divides rgb by out_a), so the source-over here multiplies
    // rgb by alpha rather than treating it as premultiplied. Gray values
    // match the layer-panel thumbnails (102/255, 153/255).
    let cell = floor(screen_pos / 8.0);
    let parity = (i32(cell.x) + i32(cell.y)) & 1;
    let checker = vec3f(select(0.6, 0.4, parity == 0));
    let composed = color.rgb * color.a + checker * (1.0 - color.a);
    return select(vec4f(composed, 1.0), view.bg, oob);
}
