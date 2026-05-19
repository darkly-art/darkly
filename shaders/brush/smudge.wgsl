// Smudge composite: drag pixels along the stroke. Per fragment, sample the
// scratch read mirror twice — once at `canvas_pos − motion` (the smear
// sample, what was under the brush at the previous dab) and once at
// `canvas_pos` (the current background, used where the dab footprint mask
// is zero). Mix toward the smear sample by `rate × mask × stroke_opacity ×
// selection`, then write the result. Where `mask == 0` the mix collapses
// to identity (writes `bg` back unchanged), so a `LoadOp::Load` scratch
// keeps everything outside the brush footprint intact.
//
// Repeated dabs along a stroke compound: each dab reads the cumulatively-
// smeared scratch from the previous dab, not the pre-stroke layer.
//
// `motion == [0, 0]` (first dab, or any stationary dab) is mathematically
// an identity write (`src == bg` → `mix(bg, bg, _) == bg`). The CPU side
// short-circuits before even issuing this pass; the shader handles it
// correctly anyway, in case that gate is ever loosened.

struct SmudgeCompositeUniforms {
    origin: vec2f,        // write quad top-left in canvas pixels
    size: vec2f,          // write quad size in canvas pixels
    target_offset: vec2f, // canvas-space offset of render target's (0,0) pixel
    target_size: vec2f,   // render target pixel dimensions (vertex NDC)
    canvas_size: vec2f,   // document canvas dimensions (fragment selection UV)
    uv_min: vec2f,        // min UV in dab texture
    uv_max: vec2f,        // max UV in dab texture
    motion: vec2f,        // per-dab delta from previous sample (canvas pixels)
    copy_origin: vec2f,   // canvas-space top-left of the scratch-mirror snapshot
    rate: f32,            // smudge rate (0 = dry, 1 = full smear)
    stroke_opacity: f32,  // per-stroke opacity cap (1.0 = no cap)
    apply_selection: u32, // 1 = modulate by selection, 0 = ignore
    _pad: u32,
}

@group(0) @binding(0) var<uniform> u: SmudgeCompositeUniforms;
@group(1) @binding(0) var t_dab: texture_2d<f32>;
@group(1) @binding(1) var s_dab: sampler;
@group(2) @binding(0) var t_selection: texture_2d<f32>;
@group(2) @binding(1) var s_selection: sampler;
@group(3) @binding(0) var t_scratch_mirror: texture_2d<f32>;
@group(3) @binding(1) var s_scratch_mirror: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) dab_uv: vec2f,
    @location(1) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Same six-vertex quad as composite.wgsl / watercolor_composite.wgsl.
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
    // Brush footprint alpha: how strongly to smear at this fragment.
    let mask = textureSample(t_dab, s_dab, in.dab_uv).a;

    // Selection mask — canvas-sized UV.
    let sel_uv = in.canvas_pos / u.canvas_size;
    let sel_raw = textureSample(t_selection, s_selection, sel_uv).r;
    let sel = select(1.0, sel_raw, u.apply_selection == 1u);

    // Both reads use the same `copy_origin` (the canvas-space top-left of
    // the mirror snapshot, sized on the CPU to cover both the dst rect
    // and the displaced src rect).
    let mirror_dims = vec2f(textureDimensions(t_scratch_mirror));
    let bg_uv = (in.canvas_pos - u.copy_origin) / mirror_dims;
    let src_uv = (in.canvas_pos - u.motion - u.copy_origin) / mirror_dims;
    let bg = textureSampleLevel(t_scratch_mirror, s_scratch_mirror, bg_uv, 0.0);
    let src = textureSampleLevel(t_scratch_mirror, s_scratch_mirror, src_uv, 0.0);

    let amount = clamp(u.rate * mask * sel * u.stroke_opacity, 0.0, 1.0);
    return mix(bg, src, amount);
}
