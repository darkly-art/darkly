// The stroke commit: lays a stroke's finished accumulations onto the layer.
//
// Two foreground slots, each with its own opacity, where an opacity of zero
// means the slot is absent, under the law `u.law` selects; the law itself is
// `lib/commit_law.wgsl`'s, shared with the appearance snapshot.
//
// A terminal maps its accumulations onto the slots; the shader knows nothing
// about brushes. One accumulation under one law is the common case (a brush
// at either end of `paint.buildup`, or watercolor), and a brush inside the
// dial fills both. The foregrounds' alpha convention is the law's (the
// deposit slots premultiplied, the move pigment straight), which is why the
// law applies the slot opacities itself; the background (the pre-stroke
// snapshot) is straight alpha, and so is the output.
//
// Two fragment entry points share one body: `fs_main` samples float
// foregrounds (an instanced terminal's `rgba8unorm` scratch and channels),
// `fs_packed` loads packed foregrounds (a compute terminal's `r32uint`
// ground, premultiplied RGBA8 in one texel). The pipeline is picked by the
// foreground format; each entry point uses only its own bindings, so the
// float and the packed layouts each match their entry.
//
// Outputs straight alpha with REPLACE blend (no hardware alpha blending).
// See docs/lessons-learned/compositing-lessons-learned.md #4 (why REPLACE).
//
// Includes `source_over.wgsl`, `lib/deposit_ceiling.wgsl` and
// `lib/commit_law.wgsl`.

struct CompositeUniforms {
    origin: vec2f,       // quad top-left in canvas pixels
    size: vec2f,         // quad size in canvas pixels
    target_offset: vec2f, // canvas-space offset of render target's (0,0) pixel
    target_size: vec2f,   // render target pixel dimensions (vertex NDC)
    uv_min: vec2f,       // min UV in the foreground textures
    uv_max: vec2f,       // max UV in the foreground textures
    blend_mode: u32,     // 0 = source-over, 1 = erase (destination-out)
    wash_opacity: f32,   // stroke opacity of the wash slot; 0 = absent
    build_opacity: f32,  // stroke opacity of the build slot; 0 = absent
    law: u32,            // 0 = deposit (wash + build), 1 = move (coverage + pigment)
}

@group(0) @binding(0) var<uniform> u: CompositeUniforms;
@group(1) @binding(0) var t_wash: texture_2d<f32>;
@group(1) @binding(1) var s_wash: sampler;
@group(2) @binding(0) var t_build: texture_2d<f32>;
@group(2) @binding(1) var s_build: sampler;
@group(1) @binding(2) var t_wash_packed: texture_2d<u32>;
@group(2) @binding(2) var t_build_packed: texture_2d<u32>;
@group(3) @binding(0) var t_bg: texture_2d<f32>;
@group(3) @binding(1) var s_bg: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) fg_uv: vec2f,
    @location(1) canvas_pos: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Quad from 6 vertices (two triangles): 0,1,2, 2,1,3
    //   0──1      unit corners: (0,0) (1,0) (0,1) (1,1)
    //   │╲ │      tri 0: 0,1,2  tri 1: 2,1,3
    //   2──3
    let corner = array<vec2f, 6>(
        vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
        vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    );
    let unit = corner[idx];
    let canvas_pos = u.origin + unit * u.size;

    // Translate canvas-space → target-local, then to NDC against target size.
    let target_local = canvas_pos - u.target_offset;
    let ndc = vec2f(
        target_local.x / u.target_size.x * 2.0 - 1.0,
        1.0 - target_local.y / u.target_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4f(ndc, 0.0, 1.0);
    out.fg_uv = u.uv_min + unit * (u.uv_max - u.uv_min);
    out.canvas_pos = canvas_pos;
    return out;
}

// Float foregrounds, sampled: the instanced terminals' scratch and channels.
@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let wash = textureSample(t_wash, s_wash, in.fg_uv);
    let build = textureSample(t_build, s_build, in.fg_uv);
    return commit_fragment(in, wash, build);
}

// Packed foregrounds, loaded at the fragment's own texel: the scratch is
// layer-sized and the quad is the layer, so the texel is exact.
@fragment fn fs_packed(in: VertexOutput) -> @location(0) vec4f {
    let px = vec2<i32>(in.position.xy);
    let wash = unpack4x8unorm(textureLoad(t_wash_packed, px, 0).r);
    let build = unpack4x8unorm(textureLoad(t_build_packed, px, 0).r);
    return commit_fragment(in, wash, build);
}

// The background under a fragment: the pre-stroke snapshot, straight
// alpha. The copy_texture_to_texture origin is floor(u.origin), integer
// pixel coords, so the floored origin maps each fragment to its own texel.
fn commit_fragment(in: VertexOutput, wash: vec4f, build: vec4f) -> vec4f {
    let copy_uv = (in.canvas_pos - floor(u.origin)) / vec2f(textureDimensions(t_bg));
    let bg = textureSample(t_bg, s_bg, copy_uv);
    return commit_law(wash, build, bg, u.blend_mode, u.law, u.wash_opacity, u.build_opacity);
}
