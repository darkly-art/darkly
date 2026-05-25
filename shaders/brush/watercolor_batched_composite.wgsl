// Watercolor (batched) composite pass — one render pass, N instanced
// quads, hardware premultiplied source-over directly onto the scratch.
//
// Each instance:
//   1. Vertex shader builds a quad in clip space covering `pos ±
//      radius * r_max_unit` (canvas pixels → layer-local → NDC).
//   2. Fragment shader runs the procedural shape mask `r(θ)` (from
//      `_shape.wgsl`), samples `t_atlas[(idx % w, idx / w)]` for the
//      pickup colour, samples selection, computes the watercolor load
//      math, and emits premultiplied `(load_rgb * fg_a, fg_a)`.
//   3. Pipeline blend state `(One, OneMinusSrcAlpha, Add)` on both
//      colour and alpha handles per-pixel source-over against the
//      scratch — atomically, in hardware ROP.
//
// Concat'd after `_shape.wgsl`, so `ShapeParams` and `shape_r_theta` are
// already in scope.

struct Dab {
    pos:           vec2<f32>,
    radius:        f32,
    r_max_unit:    f32,
    centroid:      vec2<f32>,
    softness:      f32,
    deposit:       f32,
    wetness:       f32,
    stroke_opacity: f32,
    algorithm:     u32,
    amplitude:     f32,
    frequency:     f32,
    phase:         f32,
    persistence:   f32,
    seed:          f32,
    octaves:       u32,
    n1:            f32,
    n2:            f32,
    n3:            f32,
    color:         vec4<f32>,   // straight-alpha (flow folded into .a)
}

struct CompositeUniforms {
    layer_offset:  vec2<i32>,
    layer_size:    vec2<u32>,
    canvas_size:   vec2<u32>,
    atlas_width:   u32,
    atlas_height:  u32,
}

@group(0) @binding(0) var<uniform> u: CompositeUniforms;
@group(1) @binding(0) var<storage, read> dabs: array<Dab>;
@group(2) @binding(0) var t_selection: texture_2d<f32>;
@group(2) @binding(1) var s_selection: sampler;
@group(3) @binding(0) var t_atlas: texture_2d<f32>;
@group(3) @binding(1) var s_atlas: sampler;   // unused — textureLoad

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) canvas_pos: vec2<f32>,
    @location(1) @interpolate(flat) instance_idx: u32,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];

    let dab = dabs[ii];
    let half_extent = dab.radius * dab.r_max_unit;
    let canvas_pos = dab.pos + (corner - 0.5) * 2.0 * vec2<f32>(half_extent);
    let target_local = canvas_pos - vec2<f32>(f32(u.layer_offset.x), f32(u.layer_offset.y));
    let ndc = vec2<f32>(
        target_local.x / f32(u.layer_size.x) * 2.0 - 1.0,
        1.0 - target_local.y / f32(u.layer_size.y) * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.canvas_pos = canvas_pos;
    out.instance_idx = ii;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let dab = dabs[in.instance_idx];
    let shape = ShapeParams(
        dab.algorithm,
        dab.amplitude,
        dab.frequency,
        dab.phase,
        dab.persistence,
        dab.seed,
        dab.octaves,
        dab.n1,
        dab.n2,
        dab.n3,
    );

    // Pole-relative coords in the shape's natural units. The centroid
    // translation pins the asymmetric shape's geometric centre to the
    // pen tip (mirrors `watercolor_compute.wgsl`).
    let pole_natural = (in.canvas_pos - dab.pos) / dab.radius + dab.centroid;
    let dist = length(pole_natural);
    let theta = atan2(pole_natural.y, pole_natural.x);
    let r = shape_r_theta(shape, theta);
    let softness_band = max(dab.softness, 0.004);
    let mask = 1.0 - smoothstep(r - softness_band, r, dist);
    if (mask <= 0.0) {
        discard;
    }

    // Selection mask — fragment's canvas-space UV into the selection
    // texture. Same shape as `composite.wgsl` and
    // `watercolor_composite.wgsl`.
    let sel_uv = in.canvas_pos / vec2<f32>(f32(u.canvas_size.x), f32(u.canvas_size.y));
    let sel = textureSample(t_selection, s_selection, sel_uv).r;
    if (sel <= 0.0) {
        discard;
    }

    // Pickup: read this instance's cell from the atlas. Atlas layout is
    // `(idx % atlas_width, idx / atlas_width)`. `textureLoad` doesn't
    // use the sampler.
    let atlas_x = i32(in.instance_idx % u.atlas_width);
    let atlas_y = i32(in.instance_idx / u.atlas_width);
    let pickup = textureLoad(t_atlas, vec2<i32>(atlas_x, atlas_y), 0);

    // Watercolor load math — same formulas as `watercolor_compute.wgsl`
    // and `watercolor_composite.wgsl`. Tracking RGB *and* alpha through
    // the mix is what makes deposit=0 over empty canvas a true no-op.
    let has_canvas = pickup.a > 0.05;
    let canvas_rgb = select(dab.color.rgb, pickup.rgb, has_canvas);
    let load_rgb = mix(canvas_rgb, dab.color.rgb, dab.deposit);
    let load_alpha = mix(pickup.a, dab.color.a, dab.deposit);

    let fg_a = mask * sel * dab.stroke_opacity * dab.wetness * load_alpha;
    // Premultiplied output — pipeline blend state does source-over in
    // hardware ROP: `out = src + dst * (1 - src.a)`.
    return vec4<f32>(load_rgb * fg_a, fg_a);
}
