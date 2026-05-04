// Watercolor pickup pass — alpha-weighted average of the canvas under the
// brush footprint, written to a 1×1 RGBA8 texture. The composite pass
// samples this single texel as the canvas colour the brush is sampling
// for the smudge.
//
// Each dab is independent. There is no cross-dab carry: every dab samples
// the canvas afresh. The composite shader handles all blending math
// (paint vs canvas mix by `deposit`, smudge intensity by `wetness`).
//
// Why one fragment for the whole average: the pickup is a single colour
// used at every fragment of the dab, so we want the loop to run exactly
// once per dab — not once per fragment. We render a fullscreen triangle
// into a 1×1 viewport so a single fragment produces the result.
//
// RGB is summed alpha-weighted and divided by total alpha — gives the
// COLOUR of whatever paint is in the footprint, ignoring transparent
// pixels. Without the weighting, a 10%-painted red footprint would
// average to (0.1*red + 0.9*0) = near-black dim red.
//
// Alpha is summed unweighted and divided by sample count — tracks the
// FRACTION of the footprint that has paint at all. The composite uses
// this as the load alpha for the brush, so `deposit=0` over an empty
// canvas correctly carries zero alpha (no deposit).

struct WatercolorPickupUniforms {
    center: vec2f,           // brush centre in canvas pixels
    copy_origin: vec2f,      // top-left of valid region in canvas_copy (canvas pixels)
    canvas_copy_size: vec2f, // canvas_copy texture dimensions (pixels)
    half_extent: vec2f,      // half the dab footprint (canvas pixels) per axis
}

@group(0) @binding(0) var<uniform> u: WatercolorPickupUniforms;
@group(1) @binding(0) var t_canvas_copy: texture_2d<f32>;
@group(1) @binding(1) var s_canvas_copy: sampler;

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let positions = array<vec2f, 3>(
        vec2f(-1.0, -1.0),
        vec2f( 3.0, -1.0),
        vec2f(-1.0,  3.0),
    );
    return vec4f(positions[idx], 0.0, 1.0);
}

@fragment fn fs_main() -> @location(0) vec4f {
    var sum_rgb = vec3f(0.0);
    var sum_a = 0.0;
    let n: u32 = 8u;
    let inv_n = 1.0 / f32(n);
    let count = f32(n * n);
    for (var j: u32 = 0u; j < n; j = j + 1u) {
        for (var i: u32 = 0u; i < n; i = i + 1u) {
            let cell = (vec2f(f32(i), f32(j)) + 0.5) * inv_n;
            let canvas_pos = u.center + (cell - 0.5) * 2.0 * u.half_extent;
            let copy_uv = (canvas_pos - floor(u.copy_origin)) / u.canvas_copy_size;
            let s = textureSampleLevel(t_canvas_copy, s_canvas_copy, copy_uv, 0.0);
            sum_rgb = sum_rgb + s.rgb * s.a;
            sum_a = sum_a + s.a;
        }
    }
    let avg_rgb = select(vec3f(0.0), sum_rgb / sum_a, sum_a > 0.0001);
    let avg_a = sum_a / count;
    return vec4f(avg_rgb, avg_a);
}
