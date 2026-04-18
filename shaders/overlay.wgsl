// Tool overlay shader — instanced geometry.
//
// Each overlay primitive (line, circle, rect, etc.) is one GPU instance.
// The vertex shader generates a tight bounding quad per instance.
// The fragment shader evaluates that single primitive's SDF.
//
// Two pipelines share this shader, both using standard alpha blending:
//   - Solid pipeline (fs_solid): outputs premultiplied color directly.
//   - Snapshot pipeline (fs_snapshot): samples a snapshot of the surface.
//     Branches on flags: FLAG_INVERT_COLOR thresholds luminance at 0.5
//     (white on dark, black on light); FLAG_SOFT_CONTRAST produces a
//     subtle tint toward the opposite luminance end, strength controlled
//     by mode_param.
//
// Pipeline position: drawn on top of the final surface (after present+veils)
// using LoadOp::Load. The snapshot is a GPU-to-GPU copy taken just before the
// overlay pass, only when any snapshot-sampling primitive is present.

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

struct OverlayUniforms {
    screen_size: vec2f,
    time: f32,
    _pad: f32,
    fwd_row0: vec4f,  // canvas → screen transform
    fwd_row1: vec4f,
    fwd_row2: vec4f,
    inv_row0: vec4f,  // screen → canvas transform
    inv_row1: vec4f,
    inv_row2: vec4f,
}

// 64 bytes, std430-aligned.
struct OverlayPrimitive {
    color: vec4f,         //  0: solid color (ignored for invert primitives)
    p0: vec2f,            // 16: start / center / top-left
    p1: vec2f,            // 24: end / size / bottom-right
    thickness: f32,       // 32: stroke width in screen px
    dash_len: f32,        // 36: dash period (0 = solid)
    dash_offset: f32,     // 40: animated offset for marching
    corner_radius: f32,   // 44: rounded rect radius
    kind: u32,            // 48: primitive type
    flags: u32,           // 52: bit0=canvas_space, bit1=invert, bit2=soft_contrast
    mode_param: f32,      // 56: mode-dependent scalar (e.g. soft-contrast strength)
    rotation: f32,        // 60: rotation in radians (KIND_MASKED_STAMP)
}

@group(0) @binding(0) var<uniform> u: OverlayUniforms;
@group(0) @binding(1) var<storage, read> prims: array<OverlayPrimitive>;
@group(0) @binding(2) var t_snapshot: texture_2d<f32>;
@group(0) @binding(3) var t_sampler: sampler;
@group(0) @binding(4) var t_mask: texture_2d<f32>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const KIND_LINE: u32           = 0u;
const KIND_CIRCLE: u32         = 1u;
const KIND_RECT: u32           = 2u;
const KIND_DASHED_LINE: u32    = 3u;
const KIND_FILLED_RECT: u32    = 4u;
const KIND_FILLED_CIRCLE: u32  = 5u;
const KIND_ELLIPSE: u32        = 6u;
const KIND_FILLED_ELLIPSE: u32 = 7u;
const KIND_MASKED_STAMP: u32   = 8u;

const FLAG_CANVAS_SPACE: u32   = 1u;
const FLAG_INVERT_COLOR: u32   = 2u;
const FLAG_SOFT_CONTRAST: u32  = 4u;

// ---------------------------------------------------------------------------
// Coordinate transforms
// ---------------------------------------------------------------------------

fn canvas_to_screen(p: vec2f) -> vec2f {
    return vec2f(
        u.fwd_row0.x * p.x + u.fwd_row0.y * p.y + u.fwd_row2.x,
        u.fwd_row1.x * p.x + u.fwd_row1.y * p.y + u.fwd_row2.y,
    );
}

fn screen_to_canvas(p: vec2f) -> vec2f {
    return vec2f(
        u.inv_row0.x * p.x + u.inv_row1.x * p.y + u.inv_row2.x,
        u.inv_row0.y * p.x + u.inv_row1.y * p.y + u.inv_row2.y,
    );
}

fn maybe_transform(p: vec2f, flags: u32) -> vec2f {
    if (flags & FLAG_CANVAS_SPACE) != 0u {
        return canvas_to_screen(p);
    }
    return p;
}

fn maybe_scale(r: f32, flags: u32) -> f32 {
    if (flags & FLAG_CANVAS_SPACE) != 0u {
        return r * length(vec2f(u.fwd_row0.x, u.fwd_row1.x));
    }
    return r;
}

// ---------------------------------------------------------------------------
// Vertex shader — generates bounding quad per instance
// ---------------------------------------------------------------------------

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) screen_pos: vec2f,
    @location(1) @interpolate(flat) prim_idx: u32,
}

@vertex fn vs_main(
    @builtin(vertex_index) vid: u32,
    @builtin(instance_index) iid: u32,
) -> VertexOutput {
    let prim = prims[iid];

    // Transform endpoints to screen space if needed.
    // For circles, p1 holds a radius (not a position), so scale it instead.
    let p0 = maybe_transform(prim.p0, prim.flags);
    var p1 = maybe_transform(prim.p1, prim.flags);
    let scaled_radius = maybe_scale(prim.p1.x, prim.flags);

    // Compute tight bounding box + thickness/AA margin.
    let margin = prim.thickness + 2.0;

    var lo: vec2f;
    var hi: vec2f;

    switch prim.kind {
        case KIND_CIRCLE: {
            let r = scaled_radius + margin;
            lo = p0 - vec2f(r);
            hi = p0 + vec2f(r);
        }
        case KIND_FILLED_CIRCLE: {
            let r = scaled_radius + margin;
            lo = p0 - vec2f(r);
            hi = p0 + vec2f(r);
        }
        case KIND_ELLIPSE, KIND_FILLED_ELLIPSE: {
            // p0 = center, p1 = [rx, ry]
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                let center = prim.p0;
                let radii = prim.p1;
                let c0 = canvas_to_screen(center + vec2f(-radii.x, -radii.y));
                let c1 = canvas_to_screen(center + vec2f( radii.x, -radii.y));
                let c2 = canvas_to_screen(center + vec2f(-radii.x,  radii.y));
                let c3 = canvas_to_screen(center + vec2f( radii.x,  radii.y));
                lo = min(min(c0, c1), min(c2, c3)) - vec2f(margin);
                hi = max(max(c0, c1), max(c2, c3)) + vec2f(margin);
            } else {
                lo = p0 - p1 - vec2f(margin);
                hi = p0 + p1 + vec2f(margin);
            }
        }
        case KIND_MASKED_STAMP: {
            // p0 = center, p1 = half-extent; rotation in radians.
            let c = cos(prim.rotation);
            let s = sin(prim.rotation);
            let ex = vec2f( c,  s) * prim.p1.x;
            let ey = vec2f(-s,  c) * prim.p1.y;
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                let corners = array<vec2f, 4>(
                    canvas_to_screen(prim.p0 - ex - ey),
                    canvas_to_screen(prim.p0 + ex - ey),
                    canvas_to_screen(prim.p0 - ex + ey),
                    canvas_to_screen(prim.p0 + ex + ey),
                );
                lo = min(min(corners[0], corners[1]), min(corners[2], corners[3])) - vec2f(margin);
                hi = max(max(corners[0], corners[1]), max(corners[2], corners[3])) + vec2f(margin);
            } else {
                let c0 = prim.p0 - ex - ey;
                let c1 = prim.p0 + ex - ey;
                let c2 = prim.p0 - ex + ey;
                let c3 = prim.p0 + ex + ey;
                lo = min(min(c0, c1), min(c2, c3)) - vec2f(margin);
                hi = max(max(c0, c1), max(c2, c3)) + vec2f(margin);
            }
        }
        case KIND_RECT, KIND_FILLED_RECT: {
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                // Canvas-space rect: transform all 4 corners for correct AABB.
                let c0 = canvas_to_screen(prim.p0);
                let c1 = canvas_to_screen(vec2f(prim.p1.x, prim.p0.y));
                let c2 = canvas_to_screen(vec2f(prim.p0.x, prim.p1.y));
                let c3 = canvas_to_screen(prim.p1);
                lo = min(min(c0, c1), min(c2, c3)) - vec2f(margin);
                hi = max(max(c0, c1), max(c2, c3)) + vec2f(margin);
            } else {
                lo = min(p0, p1) - vec2f(margin);
                hi = max(p0, p1) + vec2f(margin);
            }
        }
        default: {
            // Lines: AABB of the two endpoints.
            lo = min(p0, p1) - vec2f(margin);
            hi = max(p0, p1) + vec2f(margin);
        }
    }

    // Emit quad corner from vertex index (two triangles: 0,1,2, 2,1,3).
    let corner_idx = array<vec2f, 6>(
        vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
        vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    );
    let t = corner_idx[vid];
    let screen_pos = mix(lo, hi, t);

    // Screen pixels → NDC.
    var out: VertexOutput;
    out.position = vec4f(
        screen_pos / u.screen_size * 2.0 - 1.0,
        0.0,
        1.0,
    );
    out.position.y = -out.position.y;
    out.screen_pos = screen_pos;
    out.prim_idx = iid;
    return out;
}

// ---------------------------------------------------------------------------
// SDF functions (all in screen-space pixels)
// ---------------------------------------------------------------------------

fn sdf_line_segment(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let t = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * t);
}

fn sdf_circle(p: vec2f, center: vec2f, radius: f32) -> f32 {
    return abs(length(p - center) - radius);
}

fn sdf_filled_circle(p: vec2f, center: vec2f, radius: f32) -> f32 {
    return length(p - center) - radius;
}

fn sdf_ellipse(p: vec2f, center: vec2f, radii: vec2f) -> f32 {
    let d = p - center;
    let f = dot(d * d, 1.0 / (radii * radii)) - 1.0;
    let g = 2.0 * d / (radii * radii);
    let grad_len = length(g);
    if grad_len < 1e-12 { return -min(radii.x, radii.y); }
    return f / grad_len;
}

fn sdf_rect(p: vec2f, tl: vec2f, br: vec2f, corner_r: f32) -> f32 {
    let center = (tl + br) * 0.5;
    let half = (br - tl) * 0.5 - vec2f(corner_r);
    let d = abs(p - center) - half;
    return length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0) - corner_r;
}

// Parameter along line segment (for dash pattern).
fn line_param(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let ba = b - a;
    let len = length(ba);
    if len < 0.001 { return 0.0; }
    let pa = p - a;
    return clamp(dot(pa, ba) / (len * len), 0.0, 1.0) * len;
}

// ---------------------------------------------------------------------------
// Per-primitive SDF evaluation
// ---------------------------------------------------------------------------

fn eval_prim(prim: OverlayPrimitive, screen_pos: vec2f) -> f32 {
    let p0 = maybe_transform(prim.p0, prim.flags);
    let p1 = maybe_transform(prim.p1, prim.flags);
    let scaled_radius = maybe_scale(prim.p1.x, prim.flags);

    let half_t = prim.thickness * 0.5;

    var dist: f32;
    switch prim.kind {
        case KIND_LINE: {
            dist = sdf_line_segment(screen_pos, p0, p1) - half_t;
        }
        case KIND_CIRCLE: {
            dist = sdf_circle(screen_pos, p0, scaled_radius) - half_t;
        }
        case KIND_RECT: {
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                // Evaluate SDF in canvas space for correct rotation.
                let cp = screen_to_canvas(screen_pos);
                let canvas_d = sdf_rect(cp, prim.p0, prim.p1, prim.corner_radius);
                let zoom = length(vec2f(u.fwd_row0.x, u.fwd_row1.x));
                dist = abs(canvas_d) * zoom - half_t;
            } else {
                dist = abs(sdf_rect(screen_pos, p0, p1, prim.corner_radius)) - half_t;
            }
        }
        case KIND_DASHED_LINE: {
            let seg_dist = sdf_line_segment(screen_pos, p0, p1);
            dist = seg_dist - half_t;
            // Dash pattern: if in gap, discard.
            if prim.dash_len > 0.0 && dist < 1.0 {
                let t = line_param(screen_pos, p0, p1);
                let phase = (t + prim.dash_offset + u.time * 10.0) % prim.dash_len;
                if phase > prim.dash_len * 0.5 {
                    dist = 1.0; // in gap
                }
            }
        }
        case KIND_FILLED_RECT: {
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                let cp = screen_to_canvas(screen_pos);
                let canvas_d = sdf_rect(cp, prim.p0, prim.p1, prim.corner_radius);
                let zoom = length(vec2f(u.fwd_row0.x, u.fwd_row1.x));
                dist = canvas_d * zoom;
            } else {
                dist = sdf_rect(screen_pos, p0, p1, prim.corner_radius);
            }
        }
        case KIND_FILLED_CIRCLE: {
            dist = sdf_filled_circle(screen_pos, p0, scaled_radius);
        }
        case KIND_ELLIPSE: {
            // p0 = center, p1 = [rx, ry] — stroked ellipse outline
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                let cp = screen_to_canvas(screen_pos);
                let canvas_d = sdf_ellipse(cp, prim.p0, prim.p1);
                let zoom = length(vec2f(u.fwd_row0.x, u.fwd_row1.x));
                dist = abs(canvas_d) * zoom - half_t;
            } else {
                dist = abs(sdf_ellipse(screen_pos, p0, p1)) - half_t;
            }
        }
        case KIND_FILLED_ELLIPSE: {
            // p0 = center, p1 = [rx, ry] — filled ellipse (signed interior)
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                let cp = screen_to_canvas(screen_pos);
                let canvas_d = sdf_ellipse(cp, prim.p0, prim.p1);
                let zoom = length(vec2f(u.fwd_row0.x, u.fwd_row1.x));
                dist = canvas_d * zoom;
            } else {
                dist = sdf_ellipse(screen_pos, p0, p1);
            }
        }
        case KIND_MASKED_STAMP: {
            // Coverage comes directly from the mask texture. Bypass the SDF
            // smoothstep by returning the sampled value — the mask's own
            // falloff (soft brush, hard round, textured tip) is the shape.
            var local: vec2f;
            if (prim.flags & FLAG_CANVAS_SPACE) != 0u {
                local = screen_to_canvas(screen_pos) - prim.p0;
            } else {
                local = screen_pos - prim.p0;
            }
            // Inverse rotation into stamp-local space.
            let cr = cos(-prim.rotation);
            let sr = sin(-prim.rotation);
            local = vec2f(local.x * cr - local.y * sr, local.x * sr + local.y * cr);
            // UV in [0, 1]. p1 is half-extent, so divide by full extent.
            let uv = local / (prim.p1 * 2.0) + 0.5;
            if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                return 0.0;
            }
            // Red channel = grayscale coverage. Matches brush AlphaMask convention.
            return textureSampleLevel(t_mask, t_sampler, uv, 0.0).r;
        }
        default: {
            dist = 1e6;
        }
    }

    // Antialiased alpha: 1.0 inside, 0.0 outside, smooth over 1px.
    return 1.0 - smoothstep(-0.5, 0.5, dist);
}

// ---------------------------------------------------------------------------
// Fragment — solid pipeline (standard alpha blending)
// ---------------------------------------------------------------------------

@fragment fn fs_solid(in: VertexOutput) -> @location(0) vec4f {
    let prim = prims[in.prim_idx];
    let alpha = eval_prim(prim, in.screen_pos);
    if alpha < 0.001 { discard; }

    let a = prim.color.a * alpha;
    return vec4f(prim.color.rgb * a, a);
}

// ---------------------------------------------------------------------------
// Fragment — snapshot pipeline (samples surface snapshot)
//
// Two modes, branched on flags:
//   FLAG_INVERT_COLOR — luminance threshold at 0.5: dark bg → white, light
//     bg → black. Used by rect-select marching ants.
//   FLAG_SOFT_CONTRAST — subtle tint toward the opposite luminance end.
//     Strength comes from prim.mode_param (typical 0.15). Used for the
//     brush stamp preview.
// ---------------------------------------------------------------------------

@fragment fn fs_snapshot(in: VertexOutput) -> @location(0) vec4f {
    let prim = prims[in.prim_idx];
    let coverage = eval_prim(prim, in.screen_pos);
    if coverage < 0.001 { discard; }

    let uv = in.screen_pos / u.screen_size;
    let bg = textureSampleLevel(t_snapshot, t_sampler, uv, 0.0).rgb;
    let lum = dot(bg, vec3f(0.2126, 0.7152, 0.0722));

    if (prim.flags & FLAG_SOFT_CONTRAST) != 0u {
        // Soft tint: push bg toward opposite luminance end by (strength * coverage).
        // Emit alpha = coverage so the tinted interior fully replaces the
        // surface (the mix itself encodes the subtle amount).
        let tint_target = vec3f(select(0.0, 1.0, lum < 0.5));
        let rgb = mix(bg, tint_target, prim.mode_param * coverage);
        return vec4f(rgb * coverage, coverage);
    } else {
        // Invert mode: hard black/white threshold with standard alpha.
        let rgb = select(vec3f(0.0), vec3f(1.0), lum < 0.5);
        let a = prim.color.a * coverage;
        return vec4f(rgb * a, a);
    }
}
