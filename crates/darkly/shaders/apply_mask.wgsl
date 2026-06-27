// Modifier pass: modulate a window-sized projection's alpha by a mask sampled
// in the mask's OWN plane-anchored space. `proj.a *= mask_alpha`.
//
// This is the de-fused sibling of the old fused composite pass: the mask is no
// longer sampled at the host layer's UV (which coupled the two textures'
// bounds). Here the projection is window-sized (anchored at `canvas_origin`)
// and the mask carries its own offset/size as a modifier parameter, so the two
// frames are bridged explicitly: window_uv → plane → mask-local.
//
// Outside the mask footprint the result reveals (mask_alpha = 1.0), matching
// the white default — a mask only hides where it has explicit coverage. A
// `mask_size == 0` sentinel means "no footprint" → reveal everywhere. Isolated
// mode renders the mask channel as grayscale (GIMP-style debug view).

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}

@group(0) @binding(0) var t_proj: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

struct MaskUniform {
    // Mask texture's plane-space offset (top-left) and pixel size. A zero
    // `mask_size` is the "no footprint" sentinel (reveal everywhere).
    mask_offset: vec2f,
    mask_size: vec2f,
    isolated: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}
@group(0) @binding(2) var<uniform> mu: MaskUniform;

// Mask texture in its own bind group (R8Unorm).
@group(1) @binding(0) var t_mask: texture_2d<f32>;

// Shared canvas-window geometry (group 2): canvas dimensions + the plane-space
// offset of the canvas window. Same buffer bound to every composite draw.
struct CanvasUniform {
    canvas_size: vec2f,
    canvas_origin: vec2f,
}
@group(2) @binding(0) var<uniform> canvas: CanvasUniform;

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let proj = textureSample(t_proj, t_sampler, in.uv);

    // Mask sampled in its own space (window_uv → plane → mask-local), shared
    // with the passthrough-group lerp via `sample_mask_window`.
    let mask_alpha = sample_mask_window(
        t_mask,
        t_sampler,
        in.uv,
        canvas.canvas_origin,
        canvas.canvas_size,
        mu.mask_offset,
        mu.mask_size,
    );

    if (mu.isolated != 0u) {
        return vec4f(mask_alpha, mask_alpha, mask_alpha, 1.0);
    }

    return vec4f(proj.rgb, proj.a * mask_alpha);
}
