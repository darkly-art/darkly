// Chromatic aberration — shared per-pixel math for all three effect surfaces
// (destructive filter, filter layer, and veil). Works in UV space using the
// bound texture's own dimensions, so it prepends onto shaders whose binding
// layouts differ: the filter binds `src` at 0, the veil binds `t_input` at 0.
// It never names a global binding — every texture/sampler comes in as a fn
// param (the same technique `lib/canvas.wgsl`'s `sample_mask_window` uses).
// No entry points → tests/shader_compile.rs validates it as a preamble.
//
// The blur's golden-angle spiral tap distribution follows the same
// sunflower/Fibonacci construction as veils/lens_blur.wgsl.

// Array size must match `MAX_ABERRATIONS` in gpu/filters/chromatic_aberration.rs
// — the standard Rust↔WGSL uniform-layout contract.
const CA_MAX: u32 = 16u;
const CA_EPS: f32 = 1e-4;
// Golden angle ≈ 137.508° ≈ 2.39996 rad.
const CA_GA_COS: f32 = -0.7373688;
const CA_GA_SIN: f32 = 0.6754904;

// One aberration entry. Layout mirrors `GpuAberration` (32 B): a vec2 offset,
// two scalars, then a vec3 color at offset 16 (aligned) + a trailing scalar.
struct Aberration {
    offset_px: vec2f,
    scale: f32,
    blur_px: f32,
    color: vec3f,
    alpha_weight: f32,
}

// The whole effect: a channel-wise `inv_sum` normalizer, a live entry count,
// and the fixed-size entry array. Mirrors `GpuAberrationParams` (528 B).
struct AberrationParams {
    inv_sum: vec3f,
    count: u32,
    entries: array<Aberration, 16>,
}

// Sample at UV, returning transparent (`vec4f(0)`) outside [0,1] so edge content
// doesn't streak outward where users expect transparency (Step 2 boundary
// policy). Straight-alpha in, straight-alpha out. Explicit-LOD sample so it is
// valid in any control flow.
fn aberration_sample_uv(tex: texture_2d<f32>, samp: sampler, uv: vec2f) -> vec4f {
    let inside = all(uv >= vec2f(0.0)) && all(uv <= vec2f(1.0));
    let s = textureSampleLevel(tex, samp, uv, 0.0);
    return select(vec4f(0.0), s, inside);
}

// Blurred sample around `uv`: a golden-angle spiral disk of radius `blur_px`.
// Taps are premultiplied (`rgb·a`) before averaging and un-premultiplied by the
// accumulated alpha at the end — layer textures are straight-alpha, so a
// transparent tap's undefined RGB must not bleed into edges. Below ~0.25 px it
// collapses to a single tap; otherwise the tap count scales with the radius so
// large radii don't band.
fn aberration_blurred(
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2f,
    blur_px: f32,
    dims: vec2f,
) -> vec4f {
    if (blur_px < 0.25) {
        return aberration_sample_uv(tex, samp, uv);
    }
    // ≈ 8 taps per px, clamped — the schema caps blur at 6 px (≤ 48 taps).
    let taps = clamp(i32(ceil(blur_px * 8.0)), 8, 64);
    var dir = vec2f(1.0, 0.0);
    var acc_rgb = vec3f(0.0);
    var acc_a = 0.0;
    for (var n = 0; n < taps; n = n + 1) {
        dir = vec2f(
            dir.x * CA_GA_COS - dir.y * CA_GA_SIN,
            dir.x * CA_GA_SIN + dir.y * CA_GA_COS,
        );
        // sqrt radius → uniform disk density (sunflower distribution).
        let r = sqrt(f32(n) / f32(taps)) * blur_px;
        let s = aberration_sample_uv(tex, samp, uv + dir * r / dims);
        acc_rgb = acc_rgb + s.rgb * s.a;
        acc_a = acc_a + s.a;
    }
    return vec4f(acc_rgb / max(acc_a, CA_EPS), acc_a / f32(taps));
}

// The full effect at `uv`. For each entry the sample position is
// `center + (uv − center)·scale + offset` (offset px → UV via `dims`); its
// blurred sample is accumulated premultiplied and color-weighted, and
// un-premultiplied by the accumulated (weighted-average) alpha at the end. A
// zero count is an exact passthrough. `inv_sum` normalizes the color weights so
// entries whose colors sum to white at identity transforms pass through exactly.
fn aberration_apply(
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2f,
    params: AberrationParams,
) -> vec4f {
    if (params.count == 0u) {
        return aberration_sample_uv(tex, samp, uv);
    }
    let dims = vec2f(textureDimensions(tex));
    let center = vec2f(0.5, 0.5);
    var acc_rgb = vec3f(0.0); // premultiplied, color-weighted
    var acc_a = 0.0;          // weighted-average alpha (Σ alpha_weight = 1)
    let n = min(params.count, CA_MAX);
    for (var i = 0u; i < n; i = i + 1u) {
        let ab = params.entries[i];
        let src_uv = center + (uv - center) * ab.scale + ab.offset_px / dims;
        let s = aberration_blurred(tex, samp, src_uv, ab.blur_px, dims);
        acc_rgb = acc_rgb + ab.color * (s.rgb * s.a);
        acc_a = acc_a + ab.alpha_weight * s.a;
    }
    let rgb = acc_rgb * params.inv_sum / max(acc_a, CA_EPS);
    return vec4f(rgb, acc_a);
}
