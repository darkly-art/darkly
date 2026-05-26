// Common prelude for compiled brush graphs (paint_compiled and any
// future compiled terminal). Defines the intrinsic uniform struct
// shared by every assembled brush.
//
// The full shader assembled by `wgsl_compile::assemble_shader` looks
// like:
//
//   _shape.wgsl prelude
//   _compiled_prelude.wgsl (this file)
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
// Symbols defined here are referenced by every compiled brush; keep
// the surface tight.

/// Stroke-constant uniforms every compiled brush carries. Packed by
/// the terminal at the start of the uniform buffer; node-contributed
/// uniforms follow.
struct IntrinsicUniforms {
    layer_offset: vec2<i32>,
    layer_size:   vec2<u32>,
    canvas_size:  vec2<u32>,
    _pad:         vec2<u32>,
};
