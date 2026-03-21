// Stamp dab generation: samples a brush tip texture and applies color,
// opacity, rotation, mirror, and ratio transforms.
//
// Render target: RGBA8 dab texture, REPLACE blend, LoadOp::Clear(transparent).
// Viewport is set to (0, 0, dab_diameter, dab_diameter) by the host.

struct StampUniforms {
    dab_size: f32,       // actual dab diameter in pixels (matches viewport)
    opacity: f32,        // dab opacity (0-1)
    rotation: f32,       // dab rotation in radians
    ratio: f32,          // aspect ratio (1.0 = square, <1.0 = squashed)
    color: vec4f,        // RGBA paint color (straight alpha)
    mirror_x: f32,       // 1.0 = flip horizontally, 0.0 = normal
    mirror_y: f32,       // 1.0 = flip vertically, 0.0 = normal
    application: u32,    // 0=AlphaMask, 1=ImageStamp, 2=LightnessMap, 3=GradientMap
    _pad: f32,
}

@group(0) @binding(0) var<uniform> u: StampUniforms;
@group(1) @binding(0) var t_tip: texture_2d<f32>;
@group(1) @binding(1) var s_tip: sampler;

@fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let center = vec2f(u.dab_size * 0.5);

    // Transform fragment position to tip UV space:
    // 1. Center at origin
    // 2. Apply inverse rotation
    // 3. Apply inverse aspect ratio
    // 4. Apply mirror
    // 5. Map back to 0-1 UV
    var p = pos.xy - center;

    // Inverse rotation.
    let cos_r = cos(-u.rotation);
    let sin_r = sin(-u.rotation);
    p = vec2f(
        p.x * cos_r - p.y * sin_r,
        p.x * sin_r + p.y * cos_r,
    );

    // Inverse aspect ratio: stretch in Y to undo the squash.
    if u.ratio > 0.001 {
        p.y /= u.ratio;
    }

    // Map to 0-1 UV (centered).
    var uv = p / u.dab_size + 0.5;

    // Mirror.
    if u.mirror_x > 0.5 {
        uv.x = 1.0 - uv.x;
    }
    if u.mirror_y > 0.5 {
        uv.y = 1.0 - uv.y;
    }

    // Sample tip texture unconditionally (textureSample requires uniform control flow).
    // Clamp UV so the sample is valid even for out-of-bounds fragments.
    let clamped_uv = clamp(uv, vec2f(0.0), vec2f(1.0));
    let tip = textureSample(t_tip, s_tip, clamped_uv);

    // Out of bounds → transparent.
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return vec4f(0.0);
    }

    // Application mode determines how tip color maps to output.
    var out: vec4f;
    switch u.application {
        // AlphaMask: tip grayscale = opacity, color from paint color.
        case 0u: {
            let mask = tip.r; // grayscale tip — use red channel
            let a = u.color.a * mask * u.opacity;
            out = vec4f(u.color.rgb * a, a); // premultiplied
        }
        // ImageStamp: tip RGB used directly, tip alpha for opacity.
        case 1u: {
            let a = tip.a * u.opacity;
            out = vec4f(tip.rgb * a, a); // premultiplied
        }
        // LightnessMap: tip luminance modulates paint color lightness.
        case 2u: {
            let lum = dot(tip.rgb, vec3f(0.2126, 0.7152, 0.0722));
            let modulated = u.color.rgb * lum;
            let a = u.color.a * tip.a * u.opacity;
            out = vec4f(modulated * a, a); // premultiplied
        }
        // GradientMap: tip luminance used as gradient index.
        // For now, treat like alpha mask (gradient support comes later).
        default: {
            let mask = dot(tip.rgb, vec3f(0.2126, 0.7152, 0.0722));
            let a = u.color.a * mask * u.opacity;
            out = vec4f(u.color.rgb * a, a);
        }
    }

    return out;
}

// Full-screen triangle — 3 vertices cover the viewport.
@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    return vec4f(unit * 2.0 - 1.0, 0.0, 1.0);
}
