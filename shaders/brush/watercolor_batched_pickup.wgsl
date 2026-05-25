// Watercolor (batched) pickup pass — one render pass, N instances. Each
// instance writes its 1×1 alpha-weighted neighborhood average to its own
// cell in a 128×128 atlas, sampling the immutable `pre_stroke_texture`.
// The composite pass that runs after reads `t_atlas` at the same cell.
//
// Cell layout: `(instance_index % atlas_width, instance_index /
// atlas_width)`. Width is uniform (atlas dimensions live in u, so the
// shader doesn't hard-code them).
//
// Pickup numerics are bit-identical to the per-dab fragment-path
// `watercolor_pickup.wgsl`: 8×8 sampling grid across the dab footprint,
// alpha-weighted RGB / sum_a (with a `>0.0001` guard), unweighted alpha
// / 64. The only differences vs the fragment-path shader: the source
// texture is `pre_stroke_texture` rather than the live scratch
// read-mirror, and N pickups happen in one pass via per-instance dab
// records instead of one render pass per dab.

struct Dab {
    pos:           vec2<f32>,
    radius:        f32,   // natural-unit reference disc radius in canvas px
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
    color:         vec4<f32>,
}

struct PickupUniforms {
    pre_stroke_origin: vec2<i32>,   // canvas-pixel origin of pre_stroke (0,0)
    pre_stroke_size:   vec2<u32>,   // pre_stroke texture dimensions
    atlas_width:       u32,
    atlas_height:      u32,
    _pad0:             u32,
    _pad1:             u32,
}

@group(0) @binding(0) var<uniform> u: PickupUniforms;
@group(1) @binding(0) var<storage, read> dabs: array<Dab>;
@group(2) @binding(0) var t_pre_stroke: texture_2d<f32>;
@group(2) @binding(1) var s_pre_stroke: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) instance_idx: u32,
}

// Six-vertex quad emission. The quad rasterises to exactly one pixel of
// the atlas (the cell for this instance), so the fragment runs once and
// produces the pickup colour for that dab.
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

    let atlas_x = f32(ii % u.atlas_width);
    let atlas_y = f32(ii / u.atlas_width);
    let pixel = vec2<f32>(atlas_x, atlas_y) + corner;
    let aw = f32(u.atlas_width);
    let ah = f32(u.atlas_height);
    let ndc = vec2<f32>(
        pixel.x / aw * 2.0 - 1.0,
        1.0 - pixel.y / ah * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.instance_idx = ii;
    return out;
}

// 8×8 alpha-weighted average. Bit-equivalent to the fragment-path
// `watercolor_pickup.wgsl` loop — only the source texture and the
// per-instance uniform layout differ.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let dab = dabs[in.instance_idx];
    let half_extent = vec2<f32>(dab.radius * dab.r_max_unit);

    var sum_rgb = vec3<f32>(0.0);
    var sum_a = 0.0;
    let n: u32 = 8u;
    let inv_n = 1.0 / f32(n);
    let count = f32(n * n);
    let origin_f = vec2<f32>(f32(u.pre_stroke_origin.x), f32(u.pre_stroke_origin.y));
    let size_f = vec2<f32>(f32(u.pre_stroke_size.x), f32(u.pre_stroke_size.y));
    for (var j: u32 = 0u; j < n; j = j + 1u) {
        for (var i: u32 = 0u; i < n; i = i + 1u) {
            let cell = (vec2<f32>(f32(i), f32(j)) + 0.5) * inv_n;
            let canvas_pos = dab.pos + (cell - 0.5) * 2.0 * half_extent;
            // Map canvas coord into pre_stroke UV. pre_stroke_origin is
            // the canvas-pixel origin of pre_stroke (0,0). Out-of-range
            // UVs are clamped by the linear sampler (clamp-to-edge would
            // be ideal but the default canvas_copy_sampler uses
            // clamp-to-edge so we get zero-alpha edge replication, which
            // is fine — outside the layer means "no canvas paint").
            let uv = (canvas_pos - origin_f) / size_f;
            // Skip out-of-bounds samples explicitly so off-layer regions
            // contribute zero alpha (preserving the "deposit=0 over
            // empty canvas is a no-op" property).
            if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
                continue;
            }
            let s = textureSampleLevel(t_pre_stroke, s_pre_stroke, uv, 0.0);
            sum_rgb = sum_rgb + s.rgb * s.a;
            sum_a = sum_a + s.a;
        }
    }
    let avg_rgb = select(vec3<f32>(0.0), sum_rgb / sum_a, sum_a > 0.0001);
    let avg_a = sum_a / count;
    return vec4<f32>(avg_rgb, avg_a);
}
