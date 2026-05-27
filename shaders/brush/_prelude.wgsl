// Common prelude for compiled brush graphs (paint and any
// future compiled terminal). Defines the intrinsic uniform struct
// shared by every assembled brush.
//
// The full shader assembled by `wgsl_compile::assemble_shader` looks
// like:
//
//   _shape.wgsl prelude
//   _prelude.wgsl (this file)
//   struct DabRecord { ... per-node fields ... }
//   struct Uniforms  { intrinsic: IntrinsicUniforms, ... per-node ... }
//   @group bindings (uniforms, dabs storage, selection)
//   per-node decls (functions, const arrays)
//   vertex stage
//   fs_main { ... per-node bodies + terminal return ... }
//
// The per-dab bbox radius (the inflated half-extent the vertex stage
// emits the quad around, and the fragment stage discards past) lives
// on the per-dab record as `bbox_radius` — see
// `wgsl_compile::intrinsic_dab_header`. No `QUAD_R_MAX` const here:
// the value is computed per-brush by composing each node's
// `ExtentContribution` and flows through the dab record so the CPU
// and GPU sides cannot diverge.
//
// `preview_centre` / `preview_size` are written only by the preview
// render path (the hover cursor). The stroke path writes zero and
// ignores them; the preview path's vertex stage builds its single
// quad around `preview_centre ± bbox_radius` and clip-maps against
// `preview_size`.
//
// Symbols defined here are referenced by every compiled brush; keep
// the surface tight.

/// Stroke-constant uniforms every compiled brush carries. Packed by
/// the terminal at the start of the uniform buffer; node-contributed
/// uniforms follow.
struct IntrinsicUniforms {
    layer_offset:    vec2<i32>,
    layer_size:      vec2<u32>,
    canvas_size:     vec2<u32>,
    preview_centre:  vec2<f32>,
    preview_size:    vec2<u32>,
    _pad:            vec2<u32>,
};
