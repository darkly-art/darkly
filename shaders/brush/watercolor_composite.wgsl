// Watercolor compositing: stamps a dab onto the canvas using a paint color
// blended toward the canvas pickup by the user's `deposit` parameter.
//
// Two-stage blend per fragment:
//   1. mixed_rgb = mix(pickup_rgb, paint_rgb, deposit)
//        deposit = 1 → pure paint (regular stamp)
//        deposit = 0 → pure pickup (flatten to local average → smudge)
//   2. source_over(mixed_rgb premultiplied by dab.a, dab.a, canvas_copy[pixel])
//
// The dab from upstream `stamp` already bakes in its own paint color, but
// we ignore the dab's RGB — we read `dab.a` as the alpha mask only and use
// `u.paint_color.rgb` directly. That avoids de-premultiplying (undefined
// where dab.a == 0).
//
// REPLACE blend; selection masking unchanged from `composite.wgsl`. Always
// targets the RGBA8 stroke scratch — stroke→layer commit goes through the
// shared `composite.wgsl` path so this shader needs only one variant.

struct WatercolorCompositeUniforms {
    paint_color: vec4f,   // straight-alpha paint colour (rgb used; alpha ignored — comes via dab.a)
    origin: vec2f,        // quad top-left in canvas pixels
    size: vec2f,          // quad size in canvas pixels
    target_offset: vec2f, // canvas-space offset of render target's (0,0) pixel
    target_size: vec2f,   // render target pixel dimensions (vertex NDC)
    canvas_size: vec2f,   // document canvas dimensions (fragment selection UV)
    uv_min: vec2f,        // min UV in dab texture (nonzero when clipped at top/left)
    uv_max: vec2f,        // max UV in dab texture
    deposit: f32,         // paint↔pickup mix ratio (0 = pure pickup, 1 = pure paint)
    wetness: f32,         // smudge intensity (0 = dry, 1 = full smudge)
    stroke_opacity: f32,  // per-stroke opacity cap (1.0 = no cap)
    apply_selection: u32, // 1 = modulate fg by selection, 0 = ignore selection
}

@group(0) @binding(0) var<uniform> u: WatercolorCompositeUniforms;
@group(1) @binding(0) var t_dab: texture_2d<f32>;
@group(1) @binding(1) var s_dab: sampler;
@group(2) @binding(0) var t_selection: texture_2d<f32>;
@group(2) @binding(1) var s_selection: sampler;
// Combined "watercolor sources" bind group: canvas_copy (sampled) + pickup
// (loaded). Packed into one group because WebGPU caps bind groups at 4
// (groups 0..=3). The pickup binding has no sampler — `textureLoad` doesn't
// need one.
@group(3) @binding(0) var t_canvas_copy: texture_2d<f32>;
@group(3) @binding(1) var s_canvas_copy: sampler;
@group(3) @binding(2) var t_pickup: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) dab_uv: vec2f,
    @location(1) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Quad from 6 vertices (two triangles): identical to composite.wgsl.
    let corner = array<vec2f, 6>(
        vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
        vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    );
    let unit = corner[idx];
    let canvas_pos = u.origin + unit * u.size;

    let target_local = canvas_pos - u.target_offset;
    let ndc = vec2f(
        target_local.x / u.target_size.x * 2.0 - 1.0,
        1.0 - target_local.y / u.target_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.dab_uv = u.uv_min + unit * (u.uv_max - u.uv_min);
    out.canvas_pos = canvas_pos;
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Dab alpha is `mask × stamp.flow × stamp.color.a`. RGB ignored.
    let dab = textureSample(t_dab, s_dab, in.dab_uv);
    let mask = dab.a;

    // Selection masking — same shape as composite.wgsl.
    let sel_uv = in.canvas_pos / u.canvas_size;
    let sel_raw = textureSample(t_selection, s_selection, sel_uv).r;
    let sel = select(1.0, sel_raw, u.apply_selection == 1u);

    // Pickup: 1×1 alpha-weighted average of the canvas under the brush.
    // pickup.rgb = colour of the painted part of the footprint
    // pickup.a   = fraction of the footprint that has paint at all
    //
    // The brush carries a load (RGB + alpha):
    //   load_rgb   = mix(canvas_rgb, paint_rgb, deposit)
    //   load_alpha = mix(canvas_alpha, paint_alpha, deposit)
    //
    // `deposit` interpolates between pure smudge (0 = canvas only) and
    // pure paint (1 = paint only) in the brush load. Tracking alpha
    // alongside RGB is what makes deposit=0 over an empty canvas a true
    // no-op: the brush is loaded with the canvas (which has alpha=0
    // there), so there's nothing to deposit.
    //
    // `wetness` modulates how strongly the loaded brush actually disturbs
    // the canvas. wetness=0 is no effect at all (the dab passes through
    // with zero alpha); wetness=1 is full effect. Multiplying it directly
    // into fg_a keeps the gate simple and clean.
    let pickup = textureLoad(t_pickup, vec2i(0, 0), 0);
    let has_canvas = pickup.a > 0.05;
    let canvas_rgb = select(u.paint_color.rgb, pickup.rgb, has_canvas);
    let canvas_alpha = pickup.a;

    let load_rgb = mix(canvas_rgb, u.paint_color.rgb, u.deposit);
    let load_alpha = mix(canvas_alpha, u.paint_color.a, u.deposit);

    let fg_a = mask * sel * u.stroke_opacity * u.wetness * load_alpha;
    let fg_rgb_pre = load_rgb * fg_a;

    // Background: read canvas copy at this fragment's pixel — same UV
    // formulation as composite.wgsl (floor-then-divide-by-tex-dim).
    let copy_uv = (in.canvas_pos - floor(u.origin)) / vec2f(textureDimensions(t_canvas_copy));
    let bg = textureSample(t_canvas_copy, s_canvas_copy, copy_uv);

    return source_over(fg_rgb_pre, fg_a, bg);
}
