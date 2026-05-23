// Paint compute terminal — fold of circle + stamp + color_output into one
// dispatch per pen event. Drives every Basic brush (Round, Airbrush, Ink Pen).
//
// Layout
//   group(0) binding(0)  uniforms (dynamic-offset)
//   group(1) binding(0)  dab storage buffer (read)
//   group(2) binding(0)  selection texture
//   group(2) binding(1)  selection sampler
//   group(3) binding(0)  scratch storage buffer (read_write) — packed
//                        rgba8unorm as u32 per pixel, indexed
//                        `y * aligned_width + x` in layer-local coords.
//
// Threading model
//   One dispatch per phase. The dispatch grid covers the event's union
//   bbox in 8×8 tiles; each thread owns ONE pixel and walks the queued
//   dab list serially in array order, compositing in registers. One
//   scratch load at entry, one store at exit (suppressed when no dab
//   contributed). Cross-dab ordering is intrinsic to the per-thread loop
//   — no barriers, no inter-dispatch synchronization.

struct Uniforms {
    // Event union bbox in **layer-local** pixels. Defines the dispatch
    // grid; out-of-bbox lanes early-out.
    union_origin: vec2<u32>,  // (ox, oy)
    union_size:   vec2<u32>,  // (w, h)
    // Layer's canvas-space offset and pixel size. Per-dab `pos` in the
    // dab buffer is in **canvas pixels**; subtract layer_offset to get
    // layer-local.
    layer_offset: vec2<i32>,
    layer_size:   vec2<u32>,
    // Canvas size (used for selection UV).
    canvas_size:  vec2<u32>,
    // Compute buffer row pitch in pixels (= aligned bytes_per_row / 4).
    aligned_width: u32,
    dab_count:     u32,
    // 0 = paint (source-over), 1 = erase (destination-out).
    blend_mode:    u32,
    _pad:          u32,
};

struct Dab {
    // Canvas-space pen tip in pixels. Subtract `layer_offset` to convert
    // to layer-local before sampling.
    pos:        vec2<f32>,
    // Radius in canvas pixels. The dab covers `pos ± radius`.
    radius:     f32,
    // Edge softness as a fraction of radius (0 = hard, 1 = fully feathered).
    softness:   f32,
    // Premultiplied paint color (rgba). Per-dab `flow` already folded in
    // upstream so the shader can blend without scaling.
    color:      vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var<storage, read> dabs: array<Dab>;
@group(2) @binding(0) var sel_tex: texture_2d<f32>;
@group(2) @binding(1) var sel_smp: sampler;
@group(3) @binding(0) var<storage, read_write> scratch: array<u32>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(
    @builtin(workgroup_id)        wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    // Pixel in layer-local coords.
    let px = u.union_origin.x + wid.x * 8u + lid.x;
    let py = u.union_origin.y + wid.y * 8u + lid.y;
    // Out-of-bbox lanes (dispatch grid is ceil(union_size / 8), so the
    // trailing 8×8 tile straddling the bbox edge runs idle lanes).
    if (px >= u.union_origin.x + u.union_size.x ||
        py >= u.union_origin.y + u.union_size.y) {
        return;
    }
    // Defensive layer clip — the union is pre-clipped CPU-side, but
    // off-by-one would silently splat outside the scratch buffer.
    if (px >= u.layer_size.x || py >= u.layer_size.y) {
        return;
    }

    // Canvas-space sample point + selection UV are constant for this
    // thread's pixel — compute once, not per dab.
    let sample_canvas = vec2<f32>(
        f32(px) + 0.5 + f32(u.layer_offset.x),
        f32(py) + 0.5 + f32(u.layer_offset.y),
    );
    let sel_uv = sample_canvas / vec2<f32>(f32(u.canvas_size.x), f32(u.canvas_size.y));
    let sel = textureSampleLevel(sel_tex, sel_smp, sel_uv, 0.0).r;
    // Fully outside selection — no dab can paint here, so skip the
    // scratch read and the dab walk entirely.
    if (sel <= 0.0) {
        return;
    }

    let idx = py * u.aligned_width + px;
    var color = unpack4x8unorm(scratch[idx]);
    var touched = false;

    for (var d: u32 = 0u; d < u.dab_count; d = d + 1u) {
        let dab = dabs[d];
        // Cheap canvas-space AABB reject — avoids the sqrt for non-overlapping
        // dabs. Coherent within a workgroup so the branch is well-predicted.
        let dx = sample_canvas.x - dab.pos.x;
        let dy = sample_canvas.y - dab.pos.y;
        if (abs(dx) >= dab.radius || abs(dy) >= dab.radius) {
            continue;
        }
        // Disc coverage: 1 inside r_solid, falls to 0 at radius.
        let dist = sqrt(dx * dx + dy * dy);
        if (dist >= dab.radius) {
            continue;
        }
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
        coverage = coverage * sel;
        if (coverage <= 0.0) {
            continue;
        }

        // `dab.color` is premultiplied with flow folded in; `coverage`
        // scales both rgb and alpha uniformly so the foreground stays
        // premultiplied. Scratch is straight-alpha rgba8 — must use the
        // shared helpers that emit straight-alpha (see
        // `compositing-lessons-learned.md` §4).
        let src = dab.color * coverage;
        if (u.blend_mode == 1u) {
            color = destination_out(src.a, color);
        } else {
            color = source_over(src.rgb, src.a, color);
        }
        touched = true;
    }

    if (touched) {
        scratch[idx] = pack4x8unorm(color);
    }
}
