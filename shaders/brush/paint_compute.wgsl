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
//   ONE workgroup per dispatch. 8×8 = 64 threads inside it. The workgroup
//   loops over the event's queued dabs serially; for each dab it
//   tile-walks the dab's layer-local bbox in 8×8 chunks. Within a tile,
//   each thread owns one pixel and does its own load/blend/store. The
//   `storageBarrier()` between dabs guarantees dab N+1 reads what dab N
//   wrote — that's the whole point of going compute instead of fragment.

struct Uniforms {
    // Layer-clipped event union bbox in **layer-local** pixels. The
    // shader's outer loop walks this region; pixels outside the layer
    // never get touched.
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
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    // Tile-walk the event's union bbox: each iteration covers an 8×8 tile
    // and each thread handles one pixel inside it. Recomputing the bbox /
    // dab / blend per iteration is fine — the inner work is trivial.
    let tiles_x = (u.union_size.x + 7u) / 8u;
    let tiles_y = (u.union_size.y + 7u) / 8u;

    for (var d: u32 = 0u; d < u.dab_count; d = d + 1u) {
        let dab = dabs[d];

        // Dab bbox in layer-local pixels, clipped to the layer extent.
        // Off-canvas portion is dropped silently — no scratch row outside
        // [0, layer_size) is writable anyway.
        let canvas_min = dab.pos - vec2<f32>(dab.radius);
        let canvas_max = dab.pos + vec2<f32>(dab.radius);
        let local_min = canvas_min - vec2<f32>(f32(u.layer_offset.x), f32(u.layer_offset.y));
        let local_max = canvas_max - vec2<f32>(f32(u.layer_offset.x), f32(u.layer_offset.y));
        let dab_x0 = max(i32(floor(local_min.x)), i32(u.union_origin.x));
        let dab_y0 = max(i32(floor(local_min.y)), i32(u.union_origin.y));
        let dab_x1 = min(i32(ceil(local_max.x)),  i32(u.union_origin.x + u.union_size.x));
        let dab_y1 = min(i32(ceil(local_max.y)),  i32(u.union_origin.y + u.union_size.y));
        if (dab_x1 <= dab_x0 || dab_y1 <= dab_y0) {
            // Off-layer dab — nothing to do, but still hit the barrier so
            // all threads in the workgroup stay lock-step.
            storageBarrier();
            continue;
        }
        let dab_w = u32(dab_x1 - dab_x0);
        let dab_h = u32(dab_y1 - dab_y0);
        let dab_tiles_x = (dab_w + 7u) / 8u;
        let dab_tiles_y = (dab_h + 7u) / 8u;

        // Soft-circle parameters: solid out to `r_solid`, linear falloff
        // from `r_solid` to `radius`. Matches circle.wgsl's softness math
        // closely enough that the ink-pen output reads the same to the eye.
        let r_solid = dab.radius * (1.0 - dab.softness);

        for (var ty: u32 = 0u; ty < dab_tiles_y; ty = ty + 1u) {
            for (var tx: u32 = 0u; tx < dab_tiles_x; tx = tx + 1u) {
                let lx_off = tx * 8u + lid.x;
                let ly_off = ty * 8u + lid.y;
                if (lx_off >= dab_w || ly_off >= dab_h) {
                    continue;
                }
                // Layer-local pixel coords.
                let px = u32(dab_x0) + lx_off;
                let py = u32(dab_y0) + ly_off;

                // Canvas-space sample point at the pixel center.
                let sample_canvas = vec2<f32>(
                    f32(px) + 0.5 + f32(u.layer_offset.x),
                    f32(py) + 0.5 + f32(u.layer_offset.y),
                );
                let delta = sample_canvas - dab.pos;
                let dist = length(delta);

                // Disc coverage: 1 inside r_solid, falls to 0 at radius.
                var coverage: f32;
                if (dist >= dab.radius) {
                    coverage = 0.0;
                } else if (dist <= r_solid) {
                    coverage = 1.0;
                } else {
                    let t = (dab.radius - dist) / max(dab.radius - r_solid, 1e-5);
                    coverage = clamp(t, 0.0, 1.0);
                }
                if (coverage <= 0.0) {
                    continue;
                }

                // Selection modulates coverage (existing color_output
                // semantic — selection masks every dab as it lands).
                let sel_uv = vec2<f32>(
                    (f32(px) + 0.5 + f32(u.layer_offset.x)) / f32(u.canvas_size.x),
                    (f32(py) + 0.5 + f32(u.layer_offset.y)) / f32(u.canvas_size.y),
                );
                let sel = textureSampleLevel(sel_tex, sel_smp, sel_uv, 0.0).r;
                coverage = coverage * sel;
                if (coverage <= 0.0) {
                    continue;
                }

                // `dab.color` is premultiplied with flow already folded in;
                // `coverage` scales both rgb and alpha uniformly so the
                // foreground contribution stays premultiplied. The scratch
                // buffer is **straight-alpha** rgba8 — so compositing must
                // use the shared `source_over` / `destination_out` helpers
                // that emit straight-alpha output. Inlining `src + dst*(1-a)`
                // here writes premul into a straight-alpha target and
                // darkens the result. See compositing-lessons-learned.md §4.
                let src = dab.color * coverage;
                let idx = py * u.aligned_width + px;
                let dst = unpack4x8unorm(scratch[idx]);

                var blended: vec4<f32>;
                if (u.blend_mode == 1u) {
                    blended = destination_out(src.a, dst);
                } else {
                    blended = source_over(src.rgb, src.a, dst);
                }
                scratch[idx] = pack4x8unorm(blended);
            }
        }

        // Ensure dab d's writes are visible to dab d+1 within this
        // workgroup. Without this, threads that race ahead to the next
        // dab could read pre-update values.
        storageBarrier();
    }
}
