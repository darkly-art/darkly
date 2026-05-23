// Watercolor compute terminal — fold of the procedural-circle stamp,
// alpha-weighted pickup, and per-fragment composite into one compute
// dispatch per pen event. Same physical model as the fragment-path
// `watercolor` terminal:
//
//   1. PICKUP — alpha-weighted 8×8 average of the canvas under the dab
//      footprint. RGB is alpha-weighted then divided by sum-of-alpha
//      (the "colour of whatever paint is there"); alpha is unweighted
//      then divided by sample count (the "fraction of the footprint
//      that has any paint").
//   2. LOAD — the brush carries `mix(canvas, paint, deposit)`. RGB and
//      alpha mix independently, so `deposit=0` over an empty canvas
//      correctly produces zero load alpha and the dab is a no-op.
//   3. STAMP — `fg_a = mask × selection × stroke_opacity × wetness ×
//      load_alpha`; source-over blend onto the scratch.
//
// **Bit-exact pickup numerics**: the 8×8 sampling grid and the
// alpha-weighted-RGB / unweighted-alpha formulas mirror the fragment-path
// `watercolor_pickup.wgsl` exactly so the compute port reads identical
// against the fragment-path stash.
//
// Layout
//   group(0) binding(0)   uniforms (dynamic-offset)
//   group(1) binding(0)   dab storage buffer (read)
//   group(2) binding(0)   selection texture
//   group(2) binding(1)   selection sampler
//   group(3) binding(0)   scratch storage buffer (read_write) — packed
//                         rgba8unorm as u32 per pixel, indexed
//                         `y * aligned_width + x` in layer-local coords.
//
// Threading model
//   ONE workgroup per dispatch. 8×8 = 64 threads. The workgroup loops
//   over the event's queued dabs serially. Per dab:
//     a) Each thread samples ONE cell of the 8×8 pickup grid, writes
//        per-thread sums to `var<workgroup>` shared memory.
//     b) `workgroupBarrier()`. Thread 0 reduces, broadcasts via shared
//        memory.
//     c) `workgroupBarrier()` again so all threads observe the broadcast.
//     d) Tile-walk the dab's bbox in 8×8 chunks — each thread owns one
//        pixel per tile. Evaluate `r(θ)` from the shape prelude, soft-
//        disc coverage, watercolor blend, source-over into scratch.
//   `storageBarrier()` between dabs guarantees dab N+1's pickup samples
//   what dab N wrote.

struct Uniforms {
    union_origin: vec2<u32>,
    union_size:   vec2<u32>,
    layer_offset: vec2<i32>,
    layer_size:   vec2<u32>,
    canvas_size:  vec2<u32>,
    aligned_width: u32,
    dab_count:     u32,
    _pad0:         u32,
    _pad1:         u32,
}

struct Dab {
    pos:        vec2<f32>,
    radius:     f32,   // natural-unit reference disc radius in canvas px
    r_max_unit: f32,   // conservative r_max for bbox sizing (natural units)
    centroid:   vec2<f32>,
    softness:   f32,
    deposit:    f32,
    wetness:    f32,
    stroke_opacity: f32,
    algorithm:  u32,
    amplitude:  f32,
    frequency:  f32,
    phase:      f32,
    persistence: f32,
    seed:       f32,
    octaves:    u32,
    n1:         f32,
    n2:         f32,
    n3:         f32,
    color:      vec4<f32>, // straight-alpha paint colour
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var<storage, read> dabs: array<Dab>;
@group(2) @binding(0) var sel_tex: texture_2d<f32>;
@group(2) @binding(1) var sel_smp: sampler;
@group(3) @binding(0) var<storage, read_write> scratch: array<u32>;

// Per-thread pickup samples. 8×8 = 64 cells; one thread per cell.
var<workgroup> pickup_r: array<f32, 64>;
var<workgroup> pickup_g: array<f32, 64>;
var<workgroup> pickup_b: array<f32, 64>;
var<workgroup> pickup_a: array<f32, 64>;
// Broadcast slot for the reduced pickup colour (rgb = alpha-weighted avg,
// a = unweighted avg).
var<workgroup> pickup_result: vec4<f32>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
) {
    for (var d: u32 = 0u; d < u.dab_count; d = d + 1u) {
        let dab = dabs[d];
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

        // ── Dab bbox (layer-local) ───────────────────────────────────
        // Use r_max_unit so the footprint covers the worst-case
        // modulation amplitude (sine/Perlin/superformula). Pixels
        // outside this box have coverage = 0.
        let half_extent = dab.radius * dab.r_max_unit;
        let canvas_min = dab.pos - vec2<f32>(half_extent);
        let canvas_max = dab.pos + vec2<f32>(half_extent);
        let local_min = canvas_min - vec2<f32>(f32(u.layer_offset.x), f32(u.layer_offset.y));
        let local_max = canvas_max - vec2<f32>(f32(u.layer_offset.x), f32(u.layer_offset.y));
        let dab_x0 = max(i32(floor(local_min.x)), i32(u.union_origin.x));
        let dab_y0 = max(i32(floor(local_min.y)), i32(u.union_origin.y));
        let dab_x1 = min(i32(ceil(local_max.x)),  i32(u.union_origin.x + u.union_size.x));
        let dab_y1 = min(i32(ceil(local_max.y)),  i32(u.union_origin.y + u.union_size.y));
        let dab_in_layer = !(dab_x1 <= dab_x0 || dab_y1 <= dab_y0);

        // ── Pickup pass ──────────────────────────────────────────────
        // Each thread samples one cell of an 8×8 grid covering the
        // canvas footprint. Identical sampling pattern to
        // `watercolor_pickup.wgsl` so the numerics match the fragment
        // path exactly.
        let n: f32 = 8.0;
        let inv_n = 1.0 / n;
        let cell = (vec2<f32>(f32(lid.x), f32(lid.y)) + 0.5) * inv_n;
        let sample_canvas = dab.pos + (cell - 0.5) * 2.0 * vec2<f32>(half_extent);
        let sample_local = sample_canvas - vec2<f32>(f32(u.layer_offset.x), f32(u.layer_offset.y));
        let sample_xi = i32(floor(sample_local.x));
        let sample_yi = i32(floor(sample_local.y));
        var s = vec4<f32>(0.0);
        if (sample_xi >= 0 && sample_xi < i32(u.layer_size.x) &&
            sample_yi >= 0 && sample_yi < i32(u.layer_size.y)) {
            let idx = u32(sample_yi) * u.aligned_width + u32(sample_xi);
            s = unpack4x8unorm(scratch[idx]);
        }
        pickup_r[lidx] = s.r * s.a;
        pickup_g[lidx] = s.g * s.a;
        pickup_b[lidx] = s.b * s.a;
        pickup_a[lidx] = s.a;
        workgroupBarrier();

        if (lidx == 0u) {
            var sum_r = 0.0;
            var sum_g = 0.0;
            var sum_b = 0.0;
            var sum_a = 0.0;
            for (var k: u32 = 0u; k < 64u; k = k + 1u) {
                sum_r = sum_r + pickup_r[k];
                sum_g = sum_g + pickup_g[k];
                sum_b = sum_b + pickup_b[k];
                sum_a = sum_a + pickup_a[k];
            }
            // Match watercolor_pickup.wgsl exactly: alpha-weighted RGB /
            // sum_a (with the `> 0.0001` guard), unweighted alpha / 64.
            let avg_rgb = select(
                vec3<f32>(0.0),
                vec3<f32>(sum_r, sum_g, sum_b) / sum_a,
                sum_a > 0.0001,
            );
            pickup_result = vec4<f32>(avg_rgb, sum_a / 64.0);
        }
        workgroupBarrier();

        let pickup = pickup_result;

        if (!dab_in_layer) {
            // Off-layer dab — no stamp work, but still hit the barrier
            // so all threads stay lock-step for the next dab.
            storageBarrier();
            continue;
        }

        // ── Stamp pass ───────────────────────────────────────────────
        let dab_w = u32(dab_x1 - dab_x0);
        let dab_h = u32(dab_y1 - dab_y0);
        let dab_tiles_x = (dab_w + 7u) / 8u;
        let dab_tiles_y = (dab_h + 7u) / 8u;

        // Watercolor's load: mix paint with the picked-up canvas colour.
        // `deposit=0` is pure pickup (smudge); `deposit=1` is pure paint.
        // Tracking alpha alongside RGB makes `deposit=0` over empty
        // canvas a true no-op (load_alpha = 0).
        let has_canvas = pickup.a > 0.05;
        let canvas_rgb = select(dab.color.rgb, pickup.rgb, has_canvas);
        let load_rgb = mix(canvas_rgb, dab.color.rgb, dab.deposit);
        let load_alpha = mix(pickup.a, dab.color.a, dab.deposit);

        for (var ty: u32 = 0u; ty < dab_tiles_y; ty = ty + 1u) {
            for (var tx: u32 = 0u; tx < dab_tiles_x; tx = tx + 1u) {
                let lx_off = tx * 8u + lid.x;
                let ly_off = ty * 8u + lid.y;
                if (lx_off >= dab_w || ly_off >= dab_h) {
                    continue;
                }
                let px = u32(dab_x0) + lx_off;
                let py = u32(dab_y0) + ly_off;

                // Procedural shape mask: pole-relative coords in the
                // shape's natural units (where r(θ)==1 is the
                // unmodulated reference disc). The centroid translation
                // pins the asymmetric shape's geometric centre to the
                // pen tip, matching circle.wgsl's rasterisation.
                let sample_canvas_px = vec2<f32>(
                    f32(px) + 0.5 + f32(u.layer_offset.x),
                    f32(py) + 0.5 + f32(u.layer_offset.y),
                );
                let pole_natural = (sample_canvas_px - dab.pos) / dab.radius + dab.centroid;
                let dist = length(pole_natural);
                let theta = atan2(pole_natural.y, pole_natural.x);
                let r = shape_r_theta(shape, theta);
                let softness_band = max(dab.softness, 0.004);
                let mask = 1.0 - smoothstep(r - softness_band, r, dist);
                if (mask <= 0.0) {
                    continue;
                }

                let sel_uv = vec2<f32>(
                    (f32(px) + 0.5 + f32(u.layer_offset.x)) / f32(u.canvas_size.x),
                    (f32(py) + 0.5 + f32(u.layer_offset.y)) / f32(u.canvas_size.y),
                );
                let sel = textureSampleLevel(sel_tex, sel_smp, sel_uv, 0.0).r;
                if (sel <= 0.0) {
                    continue;
                }

                let fg_a = mask * sel * dab.stroke_opacity * dab.wetness * load_alpha;
                let fg_rgb_pre = load_rgb * fg_a;
                let idx = py * u.aligned_width + px;
                let dst = unpack4x8unorm(scratch[idx]);
                let blended = source_over(fg_rgb_pre, fg_a, dst);
                scratch[idx] = pack4x8unorm(blended);
            }
        }

        // Sync writes for this dab so the next dab's pickup reads them.
        storageBarrier();
    }
}
