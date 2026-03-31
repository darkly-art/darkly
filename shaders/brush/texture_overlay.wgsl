// Texture overlay: tiles a pattern image over a dab and blends it.
//
// The pattern is tiled in canvas space so the grain is consistent across
// dabs — matching Krita's `KisTextureOption::apply()` behaviour.
//
// Render target: RGBA8 dab texture, REPLACE blend, LoadOp::Clear(transparent).
// Viewport is set to (0, 0, dab_width, dab_height) by the host.

struct TexOverlayUniforms {
    dab_width: f32,       // dab viewport width in pixels
    dab_height: f32,      // dab viewport height in pixels
    position_x: f32,      // canvas X of dab center (for tiling alignment)
    position_y: f32,      // canvas Y of dab center (for tiling alignment)
    pattern_width: f32,   // pattern texture natural width in pixels
    pattern_height: f32,  // pattern texture natural height in pixels
    scale: f32,           // pattern scale (1.0 = natural size)
    strength: f32,        // blend strength (0 = no texture, 1 = full)
    blend_mode: u32,      // 0 = Multiply, 1 = Subtract, 2 = Overlay
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> u: TexOverlayUniforms;
@group(1) @binding(0) var t_dab: texture_2d<f32>;
@group(1) @binding(1) var s_dab: sampler;
@group(2) @binding(0) var t_pattern: texture_2d<f32>;
@group(2) @binding(1) var s_pattern: sampler;

@fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    // Sample the dab at this fragment.  The dab content occupies the
    // top-left dab_w × dab_h region of the pool texture (which may be
    // larger, e.g. 512×512).  Use the actual texture dimensions for UV.
    let dab_tex_size = vec2f(textureDimensions(t_dab));
    let dab_uv = pos.xy / dab_tex_size;
    let dab = textureSampleLevel(t_dab, s_dab, dab_uv, 0.0);

    // Early out: transparent dab pixels need no texture.
    if dab.a < 0.001 {
        return vec4f(0.0);
    }

    // Compute this fragment's position in canvas space.
    let canvas_pos = vec2f(
        u.position_x + (pos.x - u.dab_width * 0.5),
        u.position_y + (pos.y - u.dab_height * 0.5),
    );

    // Tile the pattern in canvas space, scaled.
    let scaled_pattern_size = vec2f(u.pattern_width, u.pattern_height) * u.scale;
    let pattern_uv = fract(canvas_pos / scaled_pattern_size);
    let pattern_sample = textureSampleLevel(t_pattern, s_pattern, pattern_uv, 0.0);

    // Convert pattern to grayscale (luminance).
    let pattern_gray = dot(pattern_sample.rgb, vec3f(0.2126, 0.7152, 0.0722));

    // Compute modulation factor based on blend mode.
    var factor: f32;
    switch u.blend_mode {
        // Multiply: dab *= pattern (most common — pencil, charcoal grain).
        case 0u: {
            factor = mix(1.0, pattern_gray, u.strength);
        }
        // Subtract: dab -= pattern (cuts into the dab, sharper effect).
        case 1u: {
            factor = max(1.0 - pattern_gray * u.strength, 0.0);
        }
        // Overlay: standard overlay blend on the modulation factor.
        default: {
            let overlay = select(
                1.0 - 2.0 * (1.0 - 0.5) * (1.0 - pattern_gray),
                2.0 * 0.5 * pattern_gray,
                pattern_gray < 0.5
            );
            factor = mix(1.0, overlay, u.strength);
        }
    }

    // Modulate the premultiplied dab uniformly — this preserves the
    // premultiplied invariant (rgb <= a) because all channels scale equally.
    return dab * factor;
}

// Full-screen triangle — 3 vertices cover the viewport.
@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    return vec4f(unit * 2.0 - 1.0, 0.0, 1.0);
}
