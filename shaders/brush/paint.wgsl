// Paint terminal — single-pass instanced fragment for Basic brushes
// (Round, Airbrush, Ink Pen). One render pass per phase, N instances
// (one per queued dab). Rasterizer assigns threads; hardware blend
// stage runs premultiplied source-over (paint) or destination-out
// (erase), depending on which pipeline the caller binds.
//
// Output convention: PREMULTIPLIED alpha. The scratch target is the
// same texture the rest of the engine treats as straight-alpha, but
// during a Basic-brush stroke the convention is internal to this
// terminal — `paint::commit` sets `fg_premultiplied: 1` so the
// composite shader interprets it correctly when blitting onto the
// layer. See `compositing-lessons-learned.md` §4 for why hardware
// source-over requires a premultiplied destination.
//
// Layout
//   group(0) binding(0)  uniforms (dynamic-offset)
//   group(1) binding(0)  dab storage buffer (read)
//   group(2) binding(0)  selection texture
//   group(2) binding(1)  selection sampler

struct Uniforms {
    // Layer-local pixel size of the scratch render target. Used to map
    // canvas-pixel quad corners into clip space.
    layer_offset: vec2<i32>,
    layer_size:   vec2<u32>,
    // Canvas size, for selection UV (selection lives in canvas space).
    canvas_size:  vec2<u32>,
    _pad:         vec2<u32>,
};

struct Dab {
    // Canvas-space pen tip in pixels.
    pos:      vec2<f32>,
    // Disc radius in canvas pixels (dab covers `pos ± radius`).
    radius:   f32,
    // Edge softness as fraction of radius (0 = hard, 1 = fully feathered).
    softness: f32,
    // Premultiplied paint color (rgba). `flow` already folded into alpha
    // upstream so the shader can multiply by coverage without extra math.
    color:    vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var<storage, read> dabs: array<Dab>;
@group(2) @binding(0) var sel_tex: texture_2d<f32>;
@group(2) @binding(1) var sel_smp: sampler;

struct VsOut {
    @builtin(position) clip:        vec4<f32>,
    // Canvas-pixel position of this fragment. Linear-interpolated.
    @location(0) canvas_pos:        vec2<f32>,
    // Which dab this instance is rendering. Flat — constant across the
    // quad's fragments.
    @location(1) @interpolate(flat) dab_idx: u32,
};

// Two-triangle quad via 6 vertices, in 0..1 quad coords.
fn quad_corner(vi: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return corners[vi];
}

@vertex
fn vs_main(
    @builtin(vertex_index)   vi: u32,
    @builtin(instance_index) ii: u32,
) -> VsOut {
    let dab = dabs[ii];
    let corner = quad_corner(vi);  // 0..1 quad coords
    // Quad in canvas-pixel space covers `pos ± radius`.
    let canvas_pos = dab.pos + (corner * 2.0 - vec2<f32>(1.0, 1.0)) * dab.radius;
    // Canvas → layer-local → clip space. The scratch render target is
    // sized to the layer; (0,0) in clip space is the layer center.
    let local = canvas_pos - vec2<f32>(f32(u.layer_offset.x), f32(u.layer_offset.y));
    let layer_w = f32(u.layer_size.x);
    let layer_h = f32(u.layer_size.y);
    let clip = vec2<f32>(
        local.x / layer_w * 2.0 - 1.0,
        1.0 - local.y / layer_h * 2.0,
    );

    var out: VsOut;
    out.clip       = vec4<f32>(clip, 0.0, 1.0);
    out.canvas_pos = canvas_pos;
    out.dab_idx    = ii;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dab = dabs[in.dab_idx];
    let dx = in.canvas_pos.x - dab.pos.x;
    let dy = in.canvas_pos.y - dab.pos.y;
    let dist = sqrt(dx * dx + dy * dy);
    if (dist >= dab.radius) {
        discard;
    }
    // Disc coverage: 1 inside r_solid, linear falloff to 0 at radius.
    // Same formula as paint_compute.wgsl so visual output matches the
    // compute-terminal baseline byte-for-byte.
    let r_solid = dab.radius * (1.0 - dab.softness);
    var coverage: f32;
    if (dist <= r_solid) {
        coverage = 1.0;
    } else {
        coverage = clamp(
            (dab.radius - dist) / max(dab.radius - r_solid, 1e-5),
            0.0, 1.0,
        );
    }
    // Selection sampled per fragment in canvas space.
    let sel_uv = in.canvas_pos / vec2<f32>(f32(u.canvas_size.x), f32(u.canvas_size.y));
    let sel = textureSampleLevel(sel_tex, sel_smp, sel_uv, 0.0).r;
    let weight = coverage * sel;
    if (weight <= 0.0) {
        discard;
    }
    // `dab.color` is premultiplied with flow folded in; scaling by a
    // scalar `weight` preserves premultiplication.
    return dab.color * weight;
}
