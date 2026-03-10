// Tool overlay shader — instanced geometry.
//
// Each overlay primitive (line, circle, rect, etc.) is one GPU instance.
// The vertex shader generates a tight bounding quad per instance.
// The fragment shader evaluates that single primitive's SDF.
//
// Two pipelines share this shader, both using standard alpha blending:
//   - Solid pipeline (fs_solid): outputs premultiplied color directly.
//   - Invert pipeline (fs_invert): samples a snapshot of the surface, computes
//     greyscale luminance, thresholds at 0.5 → white on dark, black on light.
//
// Pipeline position: drawn on top of the final surface (after present+veils)
// using LoadOp::Load. The snapshot is a GPU-to-GPU copy taken just before the
// overlay pass, only when inverted primitives are present.

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
    flags: u32,           // 52: bit0=canvas_space
    _pad0: u32,           // 56
    _pad1: u32,           // 60
}

@group(0) @binding(0) var<uniform> u: OverlayUniforms;
@group(0) @binding(1) var<storage, read> prims: array<OverlayPrimitive>;
@group(0) @binding(2) var t_snapshot: texture_2d<f32>;
@group(0) @binding(3) var t_sampler: sampler;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const KIND_LINE: u32          = 0u;
const KIND_CIRCLE: u32        = 1u;
const KIND_RECT: u32          = 2u;
const KIND_DASHED_LINE: u32   = 3u;
const KIND_FILLED_RECT: u32   = 4u;
const KIND_FILLED_CIRCLE: u32 = 5u;

const FLAG_CANVAS_SPACE: u32  = 1u;

// ---------------------------------------------------------------------------
// Coordinate transforms
// ---------------------------------------------------------------------------

fn canvas_to_screen(p: vec2f) -> vec2f {
    return vec2f(
        u.fwd_row0.x * p.x + u.fwd_row0.y * p.y + u.fwd_row2.x,
        u.fwd_row1.x * p.x + u.fwd_row1.y * p.y + u.fwd_row2.y,
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
        default: {
            // Lines and rects: AABB of the two endpoints.
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
            let inner = abs(sdf_rect(screen_pos, p0, p1, prim.corner_radius));
            dist = inner - half_t;
        }
        case KIND_DASHED_LINE: {
            let seg_dist = sdf_line_segment(screen_pos, p0, p1);
            dist = seg_dist - half_t;
            // Dash pattern: if in gap, discard.
            if prim.dash_len > 0.0 && dist < 1.0 {
                let t = line_param(screen_pos, p0, p1);
                let phase = (t + prim.dash_offset) % prim.dash_len;
                if phase > prim.dash_len * 0.5 {
                    dist = 1.0; // in gap
                }
            }
        }
        case KIND_FILLED_RECT: {
            dist = sdf_rect(screen_pos, p0, p1, prim.corner_radius);
        }
        case KIND_FILLED_CIRCLE: {
            dist = sdf_filled_circle(screen_pos, p0, scaled_radius);
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
// Fragment — invert pipeline (snapshot-based luminance threshold)
//
// Samples the surface snapshot, computes greyscale luminance, thresholds
// at 0.5: dark background → white overlay, light background → black overlay.
// ---------------------------------------------------------------------------

@fragment fn fs_invert(in: VertexOutput) -> @location(0) vec4f {
    let prim = prims[in.prim_idx];
    let alpha = eval_prim(prim, in.screen_pos);
    if alpha < 0.001 { discard; }

    let uv = in.screen_pos / u.screen_size;
    let bg = textureSampleLevel(t_snapshot, t_sampler, uv, 0.0).rgb;
    let lum = dot(bg, vec3f(0.2126, 0.7152, 0.0722));
    let rgb = select(vec3f(0.0), vec3f(1.0), lum < 0.5);

    let a = prim.color.a * alpha;
    return vec4f(rgb * a, a);
}
