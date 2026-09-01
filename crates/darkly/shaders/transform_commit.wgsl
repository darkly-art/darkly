// Transform-commit: sample a source texture through an inverse 3×3 homography
// and composite onto a layer/mask texture with shader-side Porter-Duff.
//
// The inverse matrix maps destination (canvas-local) coordinates back to
// source pixels with a per-pixel perspective divide, the same dst→src inverse
// mapping Krita (kis_perspectivetransform_worker) and GIMP (gimpdrawable-
// transform) use. Affine is the special case `inv_row2 == [0, 0, 1]`, so the
// one shader subsumes both basic and perspective transforms.
//
// The destination is copied to a temp texture before this pass runs. The shader
// reads both the transformed source and the dest copy, computes correct
// straight-alpha source-over, and outputs with REPLACE blend. This avoids the
// premultiplied-stored-as-straight bug that hardware alpha blending causes on
// straight-alpha layer textures (see compositing lessons learned #4).

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

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;

struct Uniforms {
    // Inverse homography rows: [m00, m01, m02, _pad], [m10, m11, m12, _pad],
    // [m20, m21, m22, _pad]. Affine is the special case inv_row2 = [0,0,1,_].
    inv_row0: vec4f,
    inv_row1: vec4f,
    inv_row2: vec4f,
    // Source texture origin in canvas pixel coords
    source_origin: vec2f,
    // Source texture dimensions in pixels
    source_size: vec2f,
    // Canvas-space offset of the render target's (0,0) pixel.
    target_offset: vec2f,
    // Render target pixel dimensions.
    target_size: vec2f,
    // Full document canvas dimensions in pixels.
    canvas_size: vec2f,
    opacity: f32,
    // Format flag: 0.0 = RGBA passthrough, 1.0 = R8 (output the R channel
    // straight; see the fix in fs_main).
    is_r8: f32,
}
@group(0) @binding(2) var<uniform> u: Uniforms;
@group(0) @binding(3) var t_coverage: texture_2d<f32>;

// Destination copy (straight alpha) for shader-side Porter-Duff.
@group(1) @binding(0) var t_dest: texture_2d<f32>;

struct SourceSample {
    value: vec4f,
    coverage: f32,
}

// Sample the source value and selection coverage for one destination-local
// position. Keeping them separate lets R8 represent both a selected value of
// zero and an unselected texel without ambiguity.
// The shared `proj_local` (lib/projective.wgsl) maps it back through the
// inverse homography with the perspective divide; this owns the normalization
// to a source UV and the bounds check. Returns transparent (0) outside the
// source bounds or behind the camera, so edge sub-samples average toward zero
// coverage (anti-aliasing).
fn sample_src(local: vec2f) -> SourceSample {
    let pre = proj_local(u.inv_row0, u.inv_row1, u.inv_row2, local);
    if (pre.z < 0.5) {
        return SourceSample(vec4f(0.0), 0.0);
    }
    let src_uv = pre.xy / u.source_size;
    if (any(src_uv < vec2f(0.0)) || any(src_uv >= vec2f(1.0))) {
        return SourceSample(vec4f(0.0), 0.0);
    }
    return SourceSample(
        textureSampleLevel(t_source, t_sampler, src_uv, 0.0),
        textureSampleLevel(t_coverage, t_sampler, src_uv, 0.0).r,
    );
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Convert target UV to canvas pixel position via the target's canvas-space
    // origin and size. For paste-extent layers, target_offset != 0.
    let canvas_pos = u.target_offset + in.uv * u.target_size;

    // Destination position in the source's local frame.
    let local = canvas_pos - u.source_origin;

    // 2×2 rotated-grid supersample: a single bilinear tap aliases badly on the
    // minified / far edge of a perspective warp (both Krita's worker and GIMP
    // supersample). Averaging four premultiplied taps over a rotated grid
    // anti-aliases the warped edge and also softens affine minification.
    let s0 = sample_src(local + vec2f(-0.375, -0.125));
    let s1 = sample_src(local + vec2f( 0.125, -0.375));
    let s2 = sample_src(local + vec2f( 0.375,  0.125));
    let s3 = sample_src(local + vec2f(-0.125,  0.375));
    let coverage = 0.25 * (s0.coverage + s1.coverage + s2.coverage + s3.coverage);

    if (coverage <= 0.0) {
        discard;
    }

    // Read destination (straight alpha, the layer's existing pixels).
    let bg = textureLoad(t_dest, vec2i(in.position.xy), 0);

    if (u.is_r8 > 0.5) {
        // Average value×coverage separately from coverage. Dividing to recover
        // a straight scalar and mixing it back by coverage algebraically
        // reduces to the weighted value plus the uncovered destination.
        let weighted_value = 0.25 * (
            s0.value.r * s0.coverage
            + s1.value.r * s1.coverage
            + s2.value.r * s2.coverage
            + s3.value.r * s3.coverage
        ) * u.opacity;
        let applied_coverage = coverage * u.opacity;
        let value = weighted_value + bg.r * (1.0 - applied_coverage);
        return vec4f(value, value, value, 1.0);
    } else {
        // RGBA sources are premultiplied before this pass. Apply selection
        // coverage per sub-sample, then source-over the straight destination.
        let fg_pm = 0.25 * (
            s0.value * s0.coverage
            + s1.value * s1.coverage
            + s2.value * s2.coverage
            + s3.value * s3.coverage
        );
        let fg_a = fg_pm.a * u.opacity;
        if (fg_a <= 0.0) {
            discard;
        }
        let fg_pre = fg_pm.rgb * u.opacity;
        return source_over(fg_pre, fg_a, bg);
    }
}
