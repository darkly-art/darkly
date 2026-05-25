// Common prelude for compiled brush graphs (paint_compiled and any
// future compiled terminal). Defines the intrinsic uniform struct and
// the QUAD_R_MAX const used by the generated vertex stage.
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
// Symbols defined here are referenced by every compiled brush; keep
// the surface tight.

/// Per-instance quad inflation factor. The vertex stage emits a quad
/// covering `dab.pos ± dab.radius * QUAD_R_MAX` so shape-modulating
/// nodes whose `r(θ)` exceeds 1.0 still rasterise every covered
/// fragment. 1.6 covers a circle node with `amplitude = 0.5` (the
/// slider max) with a small AA margin; if a future node wants more,
/// bump this. Overshoots a little for unmodulated discs — cheap.
const QUAD_R_MAX: f32 = 1.6;

/// Stroke-constant uniforms every compiled brush carries. Packed by
/// the terminal at the start of the uniform buffer; node-contributed
/// uniforms follow.
struct IntrinsicUniforms {
    layer_offset: vec2<i32>,
    layer_size:   vec2<u32>,
    canvas_size:  vec2<u32>,
    _pad:         vec2<u32>,
};
