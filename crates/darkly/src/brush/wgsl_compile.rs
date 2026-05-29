//! Framework for compiling brush graphs to a single WGSL fragment shader.
//!
//! At brush-load time, the compiler walks the existing `ExecutionPlan`
//! and asks each node to emit its WGSL contribution. The pieces are
//! concatenated into one shader that evaluates the whole graph per
//! fragment, per dab — no per-dab GPU dispatch, no inter-node textures.
//!
//! ## Two execution models, chosen per brush at the terminal
//!
//! A brush graph compiles its entire upstream chain into one fragment
//! shader per terminal — `circle`, `stamp`, `paint_color`, etc. fuse
//! inline, evaluated per-fragment-per-dab. No upstream per-dab GPU
//! dispatch happens.
//!
//! There is **no runtime fallback** and **no partial compilation**: a
//! brush must have every upstream node implement
//! [`BrushNodeEvaluator::compile_wgsl`] successfully, or brush load
//! fails.
//!
//! ## The compiler walk
//!
//! 1. Topology-sort via the existing [`compile`](crate::nodegraph::compile)
//!    — same `ExecStep` order the runtime dispatch uses.
//! 2. For each step, build a [`CompileWgslCtx`] with input bindings
//!    resolved against upstream output expressions (or port defaults).
//! 3. Call `evaluator.compile_wgsl(&cctx)`; abort on `Err`.
//! 4. Concatenate `decls` into module scope, `body` into `fs_main`,
//!    collect `dab_fields` + `uniform_fields`.
//! 5. Emit the final shader: prelude + uniform/dab structs + decls +
//!    fs_main wrapper that calls the terminal's emitted body.
//!
//! ## Per-dab record schema
//!
//! Each node declares the per-dab fields it needs. The compiler packs
//! them in declaration order, fronted by an intrinsic header
//! (`pos`, `radius`) every terminal reads. CPU-side, each field's
//! `pack` closure writes its bytes from the evaluator's named outputs.
//! WGSL-side, the generated `DabRecord` struct mirrors the layout.
//!
//! ## Alignment
//!
//! `vec4`/`vec2` are emitted in alignment order (largest first) within
//! each contributor's block to avoid std430 padding surprises. The CPU
//! packer asserts the total byte count matches the expected stride.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::brush::eval::BrushNodeEvaluator;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamValue;
use crate::nodegraph::{ExecutionPlan, NodeId, PortDef, PortDir, PortRef};

// ── Type system ─────────────────────────────────────────────────────────

/// WGSL scalar/vector types a node may declare for its dab fields and
/// uniform fields. Restricted to types that have natural std430 alignment
/// (no vec3 — its 16-byte alignment trips up adjacent f32 packing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WgslType {
    F32,
    U32,
    I32,
    Vec2,
    Vec4,
}

impl WgslType {
    /// Size in bytes (matches WGSL std430 size).
    pub fn size(self) -> usize {
        match self {
            Self::F32 | Self::U32 | Self::I32 => 4,
            Self::Vec2 => 8,
            Self::Vec4 => 16,
        }
    }

    /// std430 alignment in bytes.
    pub fn align(self) -> usize {
        match self {
            Self::F32 | Self::U32 | Self::I32 => 4,
            Self::Vec2 => 8,
            Self::Vec4 => 16,
        }
    }

    pub fn wgsl_name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::Vec2 => "vec2<f32>",
            Self::Vec4 => "vec4<f32>",
        }
    }
}

/// Closure that serializes one value into a byte buffer. Used for
/// both per-dab record fields and stroke-constant uniform fields —
/// the input is a name→value map the terminal builds from the
/// runner's slot table (keyed by [`CompileWgslCtx::dab_field_name`]
/// / [`CompileWgslCtx::uniform_field_name`]).
pub type ValuePacker = Arc<dyn Fn(&HashMap<String, ScalarValue>, &mut Vec<u8>) + Send + Sync>;

/// Alias for the dab-record packer (per-dab).
pub type DabPacker = ValuePacker;

/// Alias for the uniform-buffer packer (per-stroke).
pub type UniformPacker = ValuePacker;

/// One field a node contributes to the per-dab record.
#[derive(Clone)]
pub struct DabField {
    /// Field name inside the generated `DabRecord` struct. Must be
    /// unique across the graph — the compiler suffixes by node id when
    /// nodes use the helper [`CompileWgslCtx::dab_field_name`].
    pub name: String,
    pub ty: WgslType,
    /// Writes this field's value into the dab record byte buffer.
    pub pack: DabPacker,
}

impl std::fmt::Debug for DabField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DabField")
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

/// One field a node contributes to the stroke-constant uniform buffer.
#[derive(Clone)]
pub struct UniformField {
    pub name: String,
    pub ty: WgslType,
    pub pack: UniformPacker,
}

impl std::fmt::Debug for UniformField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UniformField")
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

// ── Per-node compile output ─────────────────────────────────────────────

/// What one node contributes to the compiled fragment shader.
#[derive(Default, Clone, Debug)]
pub struct NodeWgsl {
    /// Module-scope WGSL declarations: helper functions, const arrays,
    /// structs. Concatenated into the shader before `fs_main`.
    pub decls: String,
    /// Lines inserted into the `fs_main` body, in topological order.
    /// May reference: `d` (the `DabRecord`), `u` (the `Uniforms`),
    /// `local_uv: vec2<f32>` (fragment offset from dab centre, normalized
    /// so the unmodulated disc edge is at `length = 1`), `local_dist: f32`
    /// (= `length(local_uv)`), `theta: f32` (= `atan2(local_uv.y, local_uv.x)`),
    /// `target_pos: vec2<f32>` (fragment's position in the target
    /// texture's pixel space — canvas px for stroke, mask texels for
    /// preview), and any function declared in `decls` or by upstream
    /// nodes.
    pub body: String,
    /// Output port name → a WGSL expression downstream nodes substitute
    /// for that port's value. Typically a `let`-binding name introduced
    /// by `body`, but may also be a dab-field reference (`d.foo`), a
    /// uniform reference (`u.foo`), or a literal.
    pub outputs: HashMap<String, String>,
    /// Per-dab record fields this node contributes.
    pub dab_fields: Vec<DabField>,
    /// Stroke-constant uniform fields this node contributes.
    pub uniform_fields: Vec<UniformField>,
    /// Extra `@group(...) @binding(...) var ...` declarations the
    /// terminal node owns. Spliced into the assembled shader after
    /// the framework's three intrinsic bind groups (group 0: uniforms,
    /// group 1: dabs, group 2: selection). Only the terminal node
    /// should set this — the per-brush pipeline build must match the
    /// declared layout. Empty for every non-terminal node.
    ///
    /// Use case: terminals like `watercolor` need bindings
    /// the standard fragment-stage prelude doesn't provide (pickup
    /// atlas, pre-stroke canvas). Declaring them here keeps the
    /// extension scoped to the one node that uses it instead of
    /// extending the `BrushNodeEvaluator` trait surface.
    pub terminal_bindings: String,
}

// ── Input binding ───────────────────────────────────────────────────────

/// How an input port resolves when emitting WGSL.
#[derive(Clone, Debug)]
pub enum InputBinding {
    /// Port is wired to an upstream output — substitute this WGSL
    /// expression at every use site.
    Wired(String),
    /// Port is disconnected — embed this literal value as a WGSL
    /// constant.
    Default(ScalarValue),
}

impl InputBinding {
    /// Emit the WGSL expression for this binding as an `f32`. Wired
    /// expressions assumed already-f32; `Default(Scalar/Int/Bool)`
    /// emits a literal.
    pub fn as_f32(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => format!("{:.6}", v.as_f32()),
        }
    }

    /// Emit as `u32`. Coerces literals; wired exprs get a runtime cast.
    pub fn as_u32(&self) -> String {
        match self {
            Self::Wired(expr) => format!("u32({})", expr),
            Self::Default(v) => format!("{}u", v.as_f32().max(0.0) as u32),
        }
    }

    /// Emit as `vec2<f32>`.
    pub fn as_vec2(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => {
                let [x, y] = v.as_vec2();
                format!("vec2<f32>({:.6}, {:.6})", x, y)
            }
        }
    }

    /// Emit as `vec4<f32>` (color/vec4).
    pub fn as_vec4(&self) -> String {
        match self {
            Self::Wired(expr) => expr.clone(),
            Self::Default(v) => {
                let [r, g, b, a] = v.as_color();
                format!("vec4<f32>({:.6}, {:.6}, {:.6}, {:.6})", r, g, b, a)
            }
        }
    }
}

// ── Extent protocol ─────────────────────────────────────────────────────

/// One node's contribution to the per-brush dab bounding-box extent.
///
/// The framework walks the graph at brush-compile time and asks every
/// node for its `extent` contribution. Contributions are composed
/// along the graph into a single `(factor, extra_px)` pair stored on
/// [`CompiledBrush`]; the `paint` terminal multiplies the
/// per-dab effective radius by `factor` and adds `extra_px` to produce
/// the dab's `bbox_target_px`. That value is packed into the per-dab
/// record and read by the WGSL fragment shader to size the rasterized
/// quad and to clip the dab's write footprint to the layer bbox.
///
/// Because the value flows from the framework into both the CPU bbox
/// computation and the GPU shader (via the dab record), the CPU bbox
/// and shader write footprint cannot diverge. The bug this protocol
/// was introduced to fix: the WGSL prelude inflated the rasterized
/// quad by a hardcoded `QUAD_R_MAX = 1.6` while the CPU layer-clip
/// bbox used the un-inflated `radius`. On a mid-stroke save-point
/// rewind, the save-point system cleared pixels outside the CPU bbox
/// but only restored into it — so anything the shader wrote in the
/// inflation margin was lost, visibly truncating previous dabs to a
/// smaller square as the user kept drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtentContribution {
    /// No effect — bbox passes through unchanged from upstream.
    Identity,
    /// Multiplier on upstream extent. `circle` uses `1 + amp_max` for
    /// sine/perlin (or the superformula's `r_max`) so the bbox covers
    /// the shape's worst-case rasterized footprint.
    Multiply(f32),
    /// Additive canvas-pixel padding on top of upstream. Future
    /// displacement / warp nodes use this (e.g. warp by ±strength px).
    /// `passthrough` multiplies the upstream extent; `added_px` is the
    /// post-multiply additive padding in canvas pixels.
    AddCanvasPixels { passthrough: f32, added_px: f32 },
    /// Hard cap below upstream — `bbox_target_px` is min'd with
    /// `factor * radius`. For clip-to-circle style masks.
    ClipTo(f32),
}

/// Per-node context passed to `BrushNodeEvaluator::extent`. Mirrors the
/// shape of [`CompileWgslCtx`] minus the WGSL plumbing: just port defs,
/// params, and a wired-input set so [`Self::port_max_value`] can pick
/// the wire-aware max for each input.
pub struct ExtentCtx<'a> {
    pub node_id: NodeId,
    pub params: &'a [ParamValue],
    pub port_defs: &'a [PortDef<BrushWireType>],
    /// Names of input ports on this node that have an inbound wire.
    /// Used by [`Self::port_max_value`] to decide whether to return
    /// the port's `natural_range` max (wired) or its default (unwired).
    pub wired_inputs: HashSet<String>,
}

impl ExtentCtx<'_> {
    /// Maximum value the named input port can take, given the wire
    /// graph. For a wired input, returns the port's `natural_range`
    /// max (or its slider `max` if no natural range is declared) —
    /// the wire-boundary remap maps every wire to the dst's natural
    /// range, so that's the actual ceiling. For an unwired input,
    /// returns the port's `default` (the only value it can take).
    /// Unknown ports return `0.0`.
    pub fn port_max_value(&self, port_name: &str) -> f32 {
        let Some(port) = self
            .port_defs
            .iter()
            .find(|p| p.name == port_name && p.dir == PortDir::Input)
        else {
            return 0.0;
        };
        if self.wired_inputs.contains(port_name) {
            port.natural_range.map(|(_, max)| max).unwrap_or(port.max)
        } else {
            port.default
        }
    }
}

// ── Compile context ─────────────────────────────────────────────────────

/// Per-node context passed to `compile_wgsl`.
pub struct CompileWgslCtx<'a> {
    pub node_id: NodeId,
    pub params: &'a [crate::gpu::params::ParamValue],
    pub port_defs: &'a [PortDef<BrushWireType>],
    pub inputs: HashMap<String, InputBinding>,
    /// Curve LUT, if this node has a `Curve` parameter.
    pub lut: Option<&'a crate::brush::curve_math::CurveLut>,
    /// Output port names that have at least one downstream consumer
    /// in the graph. Nodes whose outputs are produced into the dab
    /// record (pen_input, random) only need to emit fields for
    /// consumed ports — unwired outputs cost nothing.
    pub consumed_outputs: HashSet<String>,
}

impl CompileWgslCtx<'_> {
    /// Look up an input binding, falling back to the port's default
    /// when disconnected. The default is materialised as a literal in
    /// the emitted WGSL.
    pub fn input(&self, name: &str) -> InputBinding {
        if let Some(b) = self.inputs.get(name) {
            return b.clone();
        }
        for port in self.port_defs {
            if port.name == name && port.dir == PortDir::Input {
                return InputBinding::Default(ScalarValue::Scalar(port.default));
            }
        }
        InputBinding::Default(ScalarValue::Scalar(0.0))
    }

    /// Returns `true` if a connected wire targets this input port
    /// (i.e. not falling through to the port default). Useful for
    /// nodes whose output depends on whether an input was supplied.
    pub fn input_is_wired(&self, name: &str) -> bool {
        matches!(self.inputs.get(name), Some(InputBinding::Wired(_)))
    }

    /// Suffix an identifier with this node's id so per-node WGSL
    /// symbols don't collide.
    pub fn ident(&self, base: &str) -> String {
        format!("{}_{}", base, self.node_id.0)
    }

    /// Suffix a dab-record field name with this node's id. Use for
    /// every per-dab field so two instances of the same node type
    /// don't collide in the generated `DabRecord` struct.
    pub fn dab_field_name(&self, base: &str) -> String {
        format!("n{}_{}", self.node_id.0, base)
    }

    /// Suffix a uniform field name with this node's id.
    pub fn uniform_field_name(&self, base: &str) -> String {
        format!("n{}_{}", self.node_id.0, base)
    }
}

// ── Shader mode ─────────────────────────────────────────────────────────

/// Which of the two compiled shader variants is being assembled.
///
/// The upstream graph contributes the same per-fragment shape /
/// color / flow expressions in both modes — only the outer skeleton
/// differs:
///
/// - **`Stroke`**: instanced quad-per-dab vertex stage; `sel` sampled
///   from a bound selection texture; terminal `@group(3)` bindings
///   (scratch mirror, pickup atlas) declared.
/// - **`Preview`**: single quad centred at `preview_centre`; `sel = 1.0`
///   inlined; no `@group(2)` selection binding, no `@group(3)`
///   terminal bindings.
///
/// The two modes share `node_decls`, `dab_layout`, and
/// `uniform_layout` — every brush stores both WGSL strings side-by-side
/// on [`CompiledBrush`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ShaderMode {
    Stroke,
    Preview,
}

// ── Intrinsic uniforms ──────────────────────────────────────────────────

/// Stroke-constant intrinsic uniforms every compiled brush carries.
/// Mirrors the WGSL `IntrinsicUniforms` defined in
/// `_prelude.wgsl` — every terminal packs this struct at the
/// front of the uniform buffer (followed by node-contributed
/// uniforms). Lives here (not on each terminal) so a layout change in
/// one place can't drift from the rest.
///
/// `preview_centre` / `preview_size` are written by the preview path
/// only; the stroke path writes zero and ignores them.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IntrinsicUniforms {
    pub layer_offset: [i32; 2],
    pub layer_size: [u32; 2],
    pub canvas_size: [u32; 2],
    pub preview_centre: [f32; 2],
    pub preview_size: [u32; 2],
    pub _pad: [u32; 2],
}

/// Size in bytes of the WGSL/Rust `IntrinsicUniforms` struct. Read by
/// the terminal-side flush path when sizing its uniform ring.
pub const INTRINSIC_UNIFORMS_SIZE: usize = std::mem::size_of::<IntrinsicUniforms>();

// ── Compiled output ─────────────────────────────────────────────────────

/// A fully compiled brush graph: WGSL source + the schemas needed to
/// pack per-dab records and stroke-constant uniforms.
#[derive(Clone)]
pub struct CompiledBrush {
    /// Full WGSL source for the brush's stroke fragment shader.
    pub stroke_wgsl: String,
    /// Full WGSL source for the brush's preview (hover-cursor) fragment
    /// shader. Same dab / uniform layouts as `stroke_wgsl`; differs
    /// only in the outer skeleton (single-quad vertex stage, `sel =
    /// 1.0`, no `@group(2)` / `@group(3)` bindings). See
    /// [`ShaderMode`].
    pub preview_wgsl: String,
    /// Per-dab record layout, in declaration order. The compiler
    /// includes the intrinsic header fields ([`INTRINSIC_DAB_FIELDS`])
    /// at the front; everything after is contributed by nodes.
    pub dab_layout: Vec<DabField>,
    /// Total per-dab record size in bytes (post-alignment padding).
    pub dab_record_size: usize,
    /// Stroke-constant uniform layout. Always includes the intrinsic
    /// terminal uniforms; node contributions follow.
    pub uniform_layout: Vec<UniformField>,
    /// Total uniform buffer size in bytes (post-padding).
    pub uniform_size: usize,
    /// Stable hash of the graph topology + relevant params, for
    /// pipeline caching.
    pub topology_hash: u64,
    /// Multiplier on per-dab `effective_radius` produced by composing
    /// every node's [`ExtentContribution::extent`] over the graph. The
    /// terminal computes `bbox_target_px = effective_radius * factor +
    /// extra_px` and packs that into the dab record's intrinsic
    /// header. `1.0` for graphs with no shape-modulating upstream
    /// (the disc fallback). See [`ExtentContribution`] for the
    /// composition rules.
    pub brush_extent_factor: f32,
    /// Additive canvas-pixel padding produced by `AddCanvasPixels`
    /// contributions (displacement / warp nodes). `0.0` for the
    /// current node set.
    pub brush_extent_extra_px: f32,
}

impl std::fmt::Debug for CompiledBrush {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledBrush")
            .field("stroke_wgsl_bytes", &self.stroke_wgsl.len())
            .field("preview_wgsl_bytes", &self.preview_wgsl.len())
            .field("dab_record_size", &self.dab_record_size)
            .field("uniform_size", &self.uniform_size)
            .field("topology_hash", &self.topology_hash)
            .finish_non_exhaustive()
    }
}

/// Errors raised when a brush graph cannot compile to WGSL.
#[derive(Debug, Clone)]
pub enum CompileError {
    /// A node's `compile_wgsl` returned `Err`. Carries the node's
    /// `type_id` and the error message for diagnostics.
    NodeNotCompilable { type_id: String, reason: String },
    /// The graph has no terminal output node (nothing produces an
    /// `rgba` value to feed the fragment shader's return).
    NoTerminal,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NodeNotCompilable { type_id, reason } => {
                write!(f, "node `{type_id}` is not WGSL-compilable: {reason}")
            }
            Self::NoTerminal => {
                write!(f, "graph has no terminal node (nothing to render)")
            }
        }
    }
}

impl std::error::Error for CompileError {}

// ── Intrinsic dab record header ─────────────────────────────────────────

/// Fields every per-dab record carries, regardless of upstream nodes:
/// dab centre (`pos`), bbox half-extent (`bbox_target_px`), and the
/// reciprocal of the dab's nominal radius (`inv_radius_target_px`).
///
/// **Invariant: the dab record describes the dab in the *target
/// texture's pixel space*.** Whichever texture the brush is rasterizing
/// into, `pos` is a coordinate in that texture's pixel grid,
/// `bbox_target_px` is a half-extent in those same pixels, and
/// `inv_radius_target_px` is `1.0 / radius_in_target_pixels`. Stroke
/// renders into the layer scratch where target px ≡ canvas px;
/// preview renders into the preview mask where target px ≢ canvas px.
/// Both paths pack a well-typed record for their target via
/// [`pack_intrinsic_dab_header`], and the WGSL is target-agnostic.
///
/// Why this matters: an earlier shape of this header carried `radius`
/// and `bbox_radius` in canvas pixels in both modes, which silently
/// broke the preview path's discard test when target ≠ canvas (the dab
/// filled the texture to a square edge on large brushes). Renaming
/// these fields to declare their frame makes the bug structurally
/// inexpressible.
///
/// `bbox_target_px` is the single source of truth for the dab's write
/// footprint — the vertex stage sizes the rasterized quad against it,
/// the fragment stage discards past it, and (in stroke mode) the CPU
/// layer-clip bbox is derived from the same value so save-points
/// cannot truncate the GPU writes.
pub fn intrinsic_dab_header() -> Vec<DabField> {
    // Order matters for std430 alignment: vec2 (8) → f32 (4) → f32 (4)
    // for total 16 bytes. The terminal packs all three via
    // `pack_intrinsic_dab_header`.
    vec![
        DabField {
            name: "pos".into(),
            ty: WgslType::Vec2,
            pack: Arc::new(|_outputs, _bytes| {
                // Terminal packs `pos` directly — placeholder packer
                // here is unused because the terminal owns this field.
                unreachable!("intrinsic pos packer should not be invoked");
            }),
        },
        DabField {
            name: "bbox_target_px".into(),
            ty: WgslType::F32,
            pack: Arc::new(|_outputs, _bytes| {
                unreachable!("intrinsic bbox_target_px packer should not be invoked");
            }),
        },
        DabField {
            name: "inv_radius_target_px".into(),
            ty: WgslType::F32,
            pack: Arc::new(|_outputs, _bytes| {
                unreachable!("intrinsic inv_radius_target_px packer should not be invoked");
            }),
        },
    ]
}

/// Compile-time number of intrinsic fields the terminal packs itself
/// before per-node fields begin. Used by the terminal's packer to skip
/// over the header.
pub const INTRINSIC_DAB_HEADER_FIELDS: usize = 3;

/// Pack the intrinsic dab header. Single source of truth — every
/// terminal's `evaluate_gpu` (stroke path) and
/// [`render_compiled_preview`] (preview path) call this. The fields
/// are interpreted in the *target texture's pixel space* (see the
/// docblock on [`intrinsic_dab_header`]). Internally inverts radius
/// once so the fragment hot path is a multiply, not a divide.
///
/// Stroke-only consumers — notably watercolor's pickup shader in
/// `watercolor.rs` — treat `1 / inv_radius_target_px` as
/// canvas-px radius. That's valid only because stroke's target ≡
/// canvas. Any new sampler that derives canvas-px sizes from the dab
/// record must restrict itself to stroke-mode dispatch.
pub fn pack_intrinsic_dab_header(
    bytes: &mut Vec<u8>,
    pos: [f32; 2],
    bbox_target_px: f32,
    radius_target_px: f32,
) {
    debug_assert!(radius_target_px > 0.0, "radius_target_px must be > 0");
    let inv_radius = 1.0 / radius_target_px.max(EPS_RADIUS_TARGET_PX);
    bytes.extend_from_slice(bytemuck::bytes_of(&pos));
    bytes.extend_from_slice(bytemuck::bytes_of(&bbox_target_px));
    bytes.extend_from_slice(bytemuck::bytes_of(&inv_radius));
}

/// Pack the intrinsic uniforms (layer offset/size, canvas size, preview
/// centre, preview size) at the front of the uniform buffer. Followed
/// by node-contributed uniforms via [`pack_uniforms`]. Single source of
/// truth; collapsed from four duplicated terminal-impl methods.
pub fn pack_intrinsic_uniforms(bytes: &mut Vec<u8>, intrinsic: IntrinsicUniforms) {
    bytes.extend_from_slice(bytemuck::bytes_of(&intrinsic));
}

// Numerical-stability floor for the target-px radius division. Not a
// physical limit — the post-scale preview radius can legitimately drop
// below 1 target px on extreme brush sizes (a sub-pixel-radius preview
// is useless anyway). The clamp prevents 1/0 / huge inv values from
// poisoning the fragment.
const EPS_RADIUS_TARGET_PX: f32 = 0.125;

/// Below this canvas-px bbox, the dab has effectively no extent and
/// `render_compiled_preview` early-returns rather than try to compute
/// a canvas-to-target scale.
const EPS_BBOX_CANVAS_PX: f32 = 1e-3;

// ── The compiler ────────────────────────────────────────────────────────

/// Compile a graph + execution plan into a [`CompiledBrush`].
///
/// `plan` must already be a topologically-sorted execution plan for
/// `graph`. The compiler walks `plan.steps`; the last step's evaluator
/// is the terminal and is responsible for emitting the `return` line
/// in `body` (its `outputs` are unused).
pub fn compile_brush_to_wgsl(
    graph: &crate::nodegraph::Graph<BrushWireType>,
    plan: &ExecutionPlan,
    evaluators: &HashMap<String, Arc<dyn BrushNodeEvaluator>>,
) -> Result<CompiledBrush, CompileError> {
    if plan.steps.is_empty() {
        return Err(CompileError::NoTerminal);
    }

    let mut decls = String::new();
    // Non-terminal node bodies — shared between the stroke and preview
    // skeletons (they're upstream of the terminal and don't depend on
    // selection or scratch/atlas bindings).
    let mut shared_body = String::new();
    // Terminal node bodies, captured per-mode. `stroke_terminal_body`
    // comes from the terminal's `compile_wgsl`; `preview_terminal_body`
    // comes from `compile_preview_body`. For terminals that don't
    // override the latter, both are the same source.
    let mut stroke_terminal_body = String::new();
    let mut preview_terminal_body = String::new();
    let mut dab_fields = intrinsic_dab_header();
    let mut uniform_fields: Vec<UniformField> = Vec::new();
    // Captured from the last (terminal) step. Spliced into the
    // stroke-mode assembled shader after the framework's three
    // intrinsic bind groups so the terminal can add its own bindings
    // (e.g. `watercolor`'s pickup atlas). Preview mode omits
    // these — the preview body doesn't sample scratch / atlas.
    let mut terminal_bindings = String::new();

    // Track each output port's emitted expression so downstream nodes
    // can substitute.
    let mut output_exprs: HashMap<PortRef, String> = HashMap::new();

    // Reverse map: slot index → PortRef. Built up as we walk steps in
    // topological order — every wire's source must already exist when
    // we encounter the dest.
    let mut slot_to_port: HashMap<usize, PortRef> = HashMap::new();

    // Pre-pass: collect every PortRef that's consumed by some
    // downstream input. Nodes use this to skip emitting dab_fields /
    // expressions for ports nothing references.
    let consumed_sources: HashSet<PortRef> = plan
        .steps
        .iter()
        .flat_map(|s| s.input_slots.iter())
        .map(|sl| sl.source.clone())
        .collect();

    for step in &plan.steps {
        let evaluator =
            evaluators
                .get(&step.type_id)
                .ok_or_else(|| CompileError::NodeNotCompilable {
                    type_id: step.type_id.clone(),
                    reason: "no evaluator registered".into(),
                })?;

        // Resolve inputs from the slot table built so far.
        let mut inputs: HashMap<String, InputBinding> = HashMap::new();
        let node = graph
            .nodes
            .get(&step.node_id)
            .expect("plan step references existing node");
        for slot_info in &step.input_slots {
            let src_port = slot_to_port.get(&slot_info.slot).cloned().or_else(|| {
                // Fall back to looking up the source from input_slots.source.
                output_exprs
                    .keys()
                    .find(|pr| **pr == slot_info.source)
                    .cloned()
            });
            let Some(src_port) = src_port else {
                continue;
            };
            let Some(expr) = output_exprs.get(&src_port).cloned() else {
                continue;
            };
            let remapped =
                apply_wire_remap(expr, &src_port, step.node_id, &slot_info.port_name, graph);
            inputs.insert(slot_info.port_name.clone(), InputBinding::Wired(remapped));
        }

        // Curve LUT (only present on nodes with a Curve param).
        let lut: Option<crate::brush::curve_math::CurveLut> =
            node.params.iter().find_map(|p| match p {
                crate::gpu::params::ParamValue::Curve(pts) if pts.len() >= 2 => {
                    Some(crate::brush::curve_math::CurveLut::from_points(pts))
                }
                _ => None,
            });

        // Collect this node's consumed output port names.
        let consumed_outputs: HashSet<String> = consumed_sources
            .iter()
            .filter(|pr| pr.node == step.node_id)
            .map(|pr| pr.port.clone())
            .collect();

        let cctx = CompileWgslCtx {
            node_id: step.node_id,
            params: &node.params,
            port_defs: &node.ports,
            inputs,
            lut: lut.as_ref(),
            consumed_outputs,
        };

        let result =
            evaluator
                .compile_wgsl(&cctx)
                .map_err(|reason| CompileError::NodeNotCompilable {
                    type_id: step.type_id.clone(),
                    reason,
                })?;

        if !result.decls.is_empty() {
            decls.push_str(&result.decls);
            if !result.decls.ends_with('\n') {
                decls.push('\n');
            }
        }
        let is_terminal = step.is_terminal;
        if !result.body.is_empty() {
            // Terminal bodies stay in their per-mode buckets; non-terminal
            // bodies are spliced into both modes.
            let target = if is_terminal {
                &mut stroke_terminal_body
            } else {
                &mut shared_body
            };
            target.push_str(&result.body);
            if !result.body.ends_with('\n') {
                target.push('\n');
            }
        }
        if is_terminal {
            // Preview body — call the terminal's preview-mode hook. The
            // default delegate returns the same NodeWgsl as `compile_wgsl`
            // (paint's stroke and preview bodies share one
            // source); watercolor/smudge/liquify override to emit a
            // neutral-color body that doesn't reference `@group(3)`.
            //
            // Only the `body` field is consumed here — decls / dab_fields
            // / uniform_fields / outputs / terminal_bindings are already
            // accumulated from the stroke pass and shared across modes
            // (helper functions a preview body references — e.g. liquify's
            // `falloff_fn` — live in `decls` and are visible to both
            // skeletons).
            let preview_result = evaluator.compile_preview_body(&cctx).map_err(|reason| {
                CompileError::NodeNotCompilable {
                    type_id: step.type_id.clone(),
                    reason,
                }
            })?;
            if !preview_result.body.is_empty() {
                preview_terminal_body.push_str(&preview_result.body);
                if !preview_result.body.ends_with('\n') {
                    preview_terminal_body.push('\n');
                }
            }
        }
        dab_fields.extend(result.dab_fields);
        uniform_fields.extend(result.uniform_fields);
        if !result.terminal_bindings.is_empty() {
            if !terminal_bindings.is_empty() {
                terminal_bindings.push('\n');
            }
            terminal_bindings.push_str(&result.terminal_bindings);
        }

        // Register this node's outputs so downstream nodes can resolve
        // their wires.
        for (port_name, slot_idx) in &step.output_slots {
            let pr = PortRef {
                node: step.node_id,
                port: port_name.clone(),
            };
            slot_to_port.insert(*slot_idx, pr.clone());
            if let Some(expr) = result.outputs.get(port_name) {
                output_exprs.insert(pr, expr.clone());
            }
        }
    }

    // Sort node-contributed dab fields by alignment-descending so
    // the std430 layout has no internal padding. The intrinsic
    // header (first `INTRINSIC_DAB_HEADER_FIELDS` entries) is
    // already aligned and stays at the front. Stable sort preserves
    // declaration order within an alignment class so individual
    // nodes' packers still see their fields in the order they
    // emitted them.
    {
        let (head, tail) = dab_fields.split_at_mut(INTRINSIC_DAB_HEADER_FIELDS);
        let _ = head;
        tail.sort_by_key(|f| std::cmp::Reverse(f.ty.align()));
    }
    // Same treatment for uniforms.
    uniform_fields.sort_by_key(|f| std::cmp::Reverse(f.ty.align()));

    // Compute per-dab record size with std430-aware alignment.
    let dab_record_size = compute_struct_size(&dab_fields);
    let uniform_size = compute_struct_size_for_uniforms(&uniform_fields);

    // Assemble the two shader variants. The non-terminal body splice
    // is identical for stroke and preview; the terminal body differs
    // (and preview drops `@group(2)` selection and `@group(3)`
    // terminal bindings).
    let stroke_body = format!("{shared_body}{stroke_terminal_body}");
    let preview_body = format!("{shared_body}{preview_terminal_body}");
    let stroke_wgsl = assemble_shader(
        ShaderMode::Stroke,
        &dab_fields,
        &uniform_fields,
        &decls,
        &stroke_body,
        &terminal_bindings,
    );
    let preview_wgsl = assemble_shader(
        ShaderMode::Preview,
        &dab_fields,
        &uniform_fields,
        &decls,
        &preview_body,
        "",
    );

    // Topology hash: stable across runs (uses DefaultHasher; if process
    // stability becomes an issue we can switch to xxhash).
    let topology_hash = hash_graph_topology(graph);

    let (brush_extent_factor, brush_extent_extra_px) =
        compose_brush_extent(graph, plan, evaluators);

    Ok(CompiledBrush {
        stroke_wgsl,
        preview_wgsl,
        dab_layout: dab_fields,
        dab_record_size,
        uniform_layout: uniform_fields,
        uniform_size,
        topology_hash,
        brush_extent_factor,
        brush_extent_extra_px,
    })
}

/// Compose every node's [`ExtentContribution`] into a single
/// `(factor, extra_px)` pair for the brush. Walks every step in the
/// execution plan in topological order; each node sees the upstream-
/// accumulated extent and contributes its own multiplier / additive
/// padding / clip. Nodes that don't override `extent` (the trait
/// default returns [`ExtentContribution::Identity`]) leave the running
/// pair unchanged.
fn compose_brush_extent(
    graph: &crate::nodegraph::Graph<BrushWireType>,
    plan: &ExecutionPlan,
    evaluators: &HashMap<String, Arc<dyn BrushNodeEvaluator>>,
) -> (f32, f32) {
    let mut factor: f32 = 1.0;
    let mut extra_px: f32 = 0.0;
    for step in &plan.steps {
        let Some(evaluator) = evaluators.get(&step.type_id) else {
            continue;
        };
        let Some(node) = graph.nodes.get(&step.node_id) else {
            continue;
        };
        let wired_inputs: HashSet<String> = step
            .input_slots
            .iter()
            .map(|s| s.port_name.clone())
            .collect();
        let ectx = ExtentCtx {
            node_id: step.node_id,
            params: &node.params,
            port_defs: &node.ports,
            wired_inputs,
        };
        match evaluator.extent(&ectx) {
            ExtentContribution::Identity => {}
            ExtentContribution::Multiply(m) => {
                factor *= m;
                extra_px *= m;
            }
            ExtentContribution::AddCanvasPixels {
                passthrough,
                added_px,
            } => {
                factor *= passthrough;
                extra_px = extra_px * passthrough + added_px;
            }
            ExtentContribution::ClipTo(cap) => {
                factor = factor.min(cap);
            }
        }
    }
    (factor, extra_px)
}

/// Pack one dab's worth of per-node values into the byte buffer the
/// terminal will upload as a storage-buffer element. The terminal
/// writes the intrinsic header (first [`INTRINSIC_DAB_HEADER_FIELDS`]
/// fields, 16 bytes) itself first, then calls this to append per-node
/// fields, then pads the buffer up to `dab_record_size`.
///
/// Each node's `compile_wgsl` is required to declare fields in
/// alignment-descending order within its contribution, and to have
/// each field's `pack` closure write exactly `field.ty.size()` bytes.
/// With those invariants this function is a straight iteration — no
/// runtime alignment dance.
pub fn pack_dab_record(
    compiled: &CompiledBrush,
    outputs: &HashMap<String, ScalarValue>,
    bytes: &mut Vec<u8>,
) {
    for field in compiled.dab_layout.iter().skip(INTRINSIC_DAB_HEADER_FIELDS) {
        let before = bytes.len();
        (field.pack)(outputs, bytes);
        debug_assert_eq!(
            bytes.len() - before,
            field.ty.size(),
            "DabField `{}` packer wrote {} bytes, expected {}",
            field.name,
            bytes.len() - before,
            field.ty.size(),
        );
    }
}

/// Shared compiled-brush preview render path. Sized, packed, and
/// dispatched identically across paint / watercolor / smudge /
/// liquify — the differences (effective_radius derivation, rotation
/// source) are caller-supplied. Returns `Some(())` on success, `None`
/// when the brush has no compiled state or the preview mask refuses
/// to allocate.
///
/// What this does:
/// 1. Grows the preview mask to fit `radius × brush_extent_factor +
///    brush_extent_extra_px` (rounded to the next power of two).
/// 2. Packs the intrinsic uniform header — `preview_centre` /
///    `preview_size` set live; `layer_offset` / `layer_size` /
///    `canvas_size` aliased to the preview mask so any node that
///    reads them in its `compile_wgsl` body sees a sane (mask-sized)
///    target.
/// 3. Packs node-contributed uniforms via [`pack_uniforms`].
/// 4. Packs one dab record at the preview centre — intrinsic header
///    (`pos`, `bbox_target_px`, `inv_radius_target_px`) plus node-
///    contributed dab fields via [`pack_dab_record`].
/// 5. Calls [`crate::brush::pipeline::BrushPipelines::render_preview`]
///    against the shared preview pipeline cache.
/// 6. Publishes [`crate::brush::eval::BrushPreviewInfo`] for the
///    overlay's `KIND_MASKED_STAMP` primitive to consume.
pub fn render_compiled_preview(
    gpu: &mut crate::brush::gpu_context::BrushGpuContext,
    radius: f32,
    rotation_rad: f32,
) -> Option<()> {
    let compiled = gpu.compiled_brush.clone()?;
    // Brush-intrinsic bbox in canvas pixels — this is the dab's
    // footprint as it will be deposited on the canvas, and what the
    // overlay quad consumes via `half_extent_canvas_px` below.
    let bbox_canvas_px = radius * compiled.brush_extent_factor + compiled.brush_extent_extra_px;
    let (target_view, target_w, target_h) = gpu.ensure_preview_mask(bbox_canvas_px)?;
    if target_w == 0 || target_h == 0 || bbox_canvas_px < EPS_BBOX_CANVAS_PX {
        return None;
    }

    // Map canvas-px intrinsic frame → texel frame. The dab's bbox
    // unconditionally fills the inscribed half-side of the preview
    // mask; the radius scales by the same ratio so the fragment's
    // `local_uv = local * inv_radius_target_px` is dimensionless and
    // matches the value the stroke pass would produce at the same
    // intrinsic point. The overlay's displayed quad still spans
    // `±bbox_canvas_px`, so UV [0, 1] across that quad maps to UV [0, 1]
    // across the dab content in the mask.
    let texture_half = (target_w.min(target_h) as f32) * 0.5;
    let canvas_to_target = texture_half / bbox_canvas_px;
    let bbox_target_px = texture_half;
    let radius_target_px = (radius * canvas_to_target).max(EPS_RADIUS_TARGET_PX);
    let preview_centre = [target_w as f32 * 0.5, target_h as f32 * 0.5];

    // Pack the uniform buffer: intrinsic header first, node-contributed
    // uniforms after.
    let intrinsic = IntrinsicUniforms {
        layer_offset: [0, 0],
        layer_size: [target_w, target_h],
        canvas_size: [target_w, target_h],
        preview_centre,
        preview_size: [target_w, target_h],
        _pad: [0, 0],
    };
    let total_uniform_size = INTRINSIC_UNIFORMS_SIZE + compiled.uniform_size;
    let mut uniform_bytes: Vec<u8> = Vec::with_capacity(total_uniform_size);
    pack_intrinsic_uniforms(&mut uniform_bytes, intrinsic);
    let empty_outputs;
    let outputs = match gpu.slot_outputs_owned.as_ref() {
        Some(o) => o,
        None => {
            empty_outputs = HashMap::new();
            &empty_outputs
        }
    };
    pack_uniforms(&compiled, outputs, &mut uniform_bytes);
    if uniform_bytes.len() < total_uniform_size {
        uniform_bytes.resize(total_uniform_size, 0);
    }

    // Pack the single preview dab record: intrinsic header + node
    // fields. The header is in *target-pixel* space (preview mask
    // texels), so the vertex/fragment math is unit-coherent against
    // the preview target without needing any mode awareness.
    let mut dab_bytes: Vec<u8> = Vec::with_capacity(compiled.dab_record_size);
    pack_intrinsic_dab_header(
        &mut dab_bytes,
        preview_centre,
        bbox_target_px,
        radius_target_px,
    );
    pack_dab_record(&compiled, outputs, &mut dab_bytes);
    if dab_bytes.len() < compiled.dab_record_size {
        dab_bytes.resize(compiled.dab_record_size, 0);
    }

    gpu.pipelines.render_preview(
        gpu.device,
        gpu.queue,
        &mut gpu.encoder,
        &compiled,
        &target_view,
        (target_w, target_h),
        &uniform_bytes,
        &dab_bytes,
    );

    // The overlay consumer expects canvas px — its displayed quad
    // spans `±half_extent_canvas_px`, and the mask sampler maps
    // UV [0, 1] across the quad. With the dab filling the mask's
    // inscribed disc by construction (above), this matches.
    gpu.brush_preview_info = Some(crate::brush::eval::BrushPreviewInfo {
        half_extent_canvas_px: [bbox_canvas_px, bbox_canvas_px],
        rotation_rad,
    });
    Some(())
}

/// Pack the node-contributed portion of the uniform buffer. The
/// terminal packs the intrinsic header (`IntrinsicUniforms`) itself
/// before calling this.
pub fn pack_uniforms(
    compiled: &CompiledBrush,
    outputs: &HashMap<String, ScalarValue>,
    bytes: &mut Vec<u8>,
) {
    for field in &compiled.uniform_layout {
        let before = bytes.len();
        (field.pack)(outputs, bytes);
        debug_assert_eq!(
            bytes.len() - before,
            field.ty.size(),
            "UniformField `{}` packer wrote {} bytes, expected {}",
            field.name,
            bytes.len() - before,
            field.ty.size(),
        );
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn align_to(value: usize, alignment: usize) -> usize {
    if alignment == 0 {
        return value;
    }
    (value + alignment - 1) & !(alignment - 1)
}

fn compute_struct_size(fields: &[DabField]) -> usize {
    let mut size = 0;
    let mut max_align = 4;
    for f in fields {
        size = align_to(size, f.ty.align());
        size += f.ty.size();
        max_align = max_align.max(f.ty.align());
    }
    align_to(size, max_align)
}

fn compute_struct_size_for_uniforms(fields: &[UniformField]) -> usize {
    let mut size = 0;
    let mut max_align = 4;
    for f in fields {
        size = align_to(size, f.ty.align());
        size += f.ty.size();
        max_align = max_align.max(f.ty.align());
    }
    align_to(size, max_align)
}

/// Wire-boundary scalar remap, mirroring [`crate::brush::eval`]'s
/// `remap_for_wire` but emitted as a WGSL expression. When both ends of
/// a connection declare `natural_range`, we wrap the source expression
/// in an affine map from src to dst range. Otherwise the expression
/// passes through.
fn apply_wire_remap(
    expr: String,
    source: &PortRef,
    dest_node: NodeId,
    dest_port: &str,
    graph: &crate::nodegraph::Graph<BrushWireType>,
) -> String {
    let src_range = graph
        .nodes
        .get(&source.node)
        .and_then(|n| {
            n.ports
                .iter()
                .find(|p| p.name == source.port && p.dir == PortDir::Output)
        })
        .and_then(|p| p.natural_range);
    let dst_range = graph
        .nodes
        .get(&dest_node)
        .and_then(|n| {
            n.ports
                .iter()
                .find(|p| p.name == dest_port && p.dir == PortDir::Input)
        })
        .and_then(|p| p.natural_range);
    let (Some((src_min, src_max)), Some((dst_min, dst_max))) = (src_range, dst_range) else {
        return expr;
    };
    if (src_min - dst_min).abs() < 1e-6 && (src_max - dst_max).abs() < 1e-6 {
        return expr;
    }
    let denom = src_max - src_min;
    if denom.abs() < 1e-6 {
        return format!("{:.6}", dst_min);
    }
    let scale = (dst_max - dst_min) / denom;
    let bias = dst_min - src_min * scale;
    // `(expr) * scale + bias`
    format!("(({}) * {:.6} + {:.6})", expr, scale, bias)
}

fn hash_graph_topology(graph: &crate::nodegraph::Graph<BrushWireType>) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    let mut node_ids: Vec<_> = graph.nodes.keys().copied().collect();
    node_ids.sort_by_key(|n| n.0);
    for id in &node_ids {
        let node = &graph.nodes[id];
        id.0.hash(&mut hasher);
        node.type_id.hash(&mut hasher);
        // Hash params by serialising — order is stable; values that
        // affect compilation (algorithm enum, curve points) end up in
        // the hash.
        if let Ok(s) = serde_json::to_string(&node.params) {
            s.hash(&mut hasher);
        }
        for port in &node.ports {
            port.name.hash(&mut hasher);
            port.default.to_bits().hash(&mut hasher);
        }
    }
    let mut conns: Vec<_> = graph.connections.iter().collect();
    conns.sort_by_key(|c| {
        (
            c.from.node.0,
            c.from.port.clone(),
            c.to.node.0,
            c.to.port.clone(),
        )
    });
    for c in conns {
        c.from.node.0.hash(&mut hasher);
        c.from.port.hash(&mut hasher);
        c.to.node.0.hash(&mut hasher);
        c.to.port.hash(&mut hasher);
    }
    hasher.finish()
}

fn assemble_shader(
    mode: ShaderMode,
    dab_fields: &[DabField],
    uniform_fields: &[UniformField],
    node_decls: &str,
    fs_body: &str,
    terminal_bindings: &str,
) -> String {
    let mut out = String::new();
    out.push_str(include_str!("../../../../shaders/brush/_shape.wgsl"));
    out.push('\n');
    out.push_str(include_str!("../../../../shaders/brush/_prelude.wgsl"));
    out.push('\n');

    // Generated DabRecord struct.
    out.push_str("struct DabRecord {\n");
    for f in dab_fields {
        out.push_str(&format!("    {}: {},\n", f.name, f.ty.wgsl_name()));
    }
    out.push_str("};\n\n");

    // Generated Uniforms struct (always has the intrinsic terminal
    // uniforms, defined in _prelude.wgsl as
    // `IntrinsicUniforms`).
    if uniform_fields.is_empty() {
        out.push_str("struct Uniforms {\n");
        out.push_str("    intrinsic: IntrinsicUniforms,\n");
        out.push_str("};\n\n");
    } else {
        out.push_str("struct Uniforms {\n");
        out.push_str("    intrinsic: IntrinsicUniforms,\n");
        for f in uniform_fields {
            out.push_str(&format!("    {}: {},\n", f.name, f.ty.wgsl_name()));
        }
        out.push_str("};\n\n");
    }

    // Bind groups: group(0) = uniforms (both modes), group(1) = dabs
    // storage (both modes). In stroke mode group(2) = selection and
    // optional terminal `@group(3)` bindings. Preview mode omits both
    // — the skeleton hard-codes `sel = 1.0` and the preview body never
    // samples scratch / atlas.
    out.push_str("@group(0) @binding(0) var<uniform> u: Uniforms;\n");
    out.push_str("@group(1) @binding(0) var<storage, read> dabs: array<DabRecord>;\n");
    if mode == ShaderMode::Stroke {
        out.push_str("@group(2) @binding(0) var sel_tex: texture_2d<f32>;\n");
        out.push_str("@group(2) @binding(1) var sel_smp: sampler;\n");
        if !terminal_bindings.is_empty() {
            out.push_str(terminal_bindings);
            if !terminal_bindings.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out.push('\n');

    // Node-level declarations (helper functions, const arrays).
    out.push_str(node_decls);
    out.push('\n');

    // Vertex stage — paint.wgsl-style instanced quad in stroke mode,
    // single quad at `dab.pos ± dab.bbox_target_px` mapped into the
    // preview-mask viewport in preview mode.
    match mode {
        ShaderMode::Stroke => out.push_str(STROKE_VERTEX_STAGE_WGSL),
        ShaderMode::Preview => out.push_str(PREVIEW_VERTEX_STAGE_WGSL),
    }
    out.push('\n');

    // Fragment stage — header binds the fragment-local helpers, then
    // splices in the node bodies, then ends with the terminal's
    // `return` line (emitted into `fs_body`). The `sel` binding line
    // differs between modes: stroke samples a real texture, preview
    // hard-codes 1.0 (the full footprint, ignoring any active
    // selection — matches master's preview behavior).
    out.push_str("@fragment\n");
    out.push_str("fn fs_main(in: VsOut) -> @location(0) vec4<f32> {\n");
    out.push_str("    let d = dabs[in.dab_idx];\n");
    // `target_pos` is in the target texture's pixel space — canvas px
    // for stroke (target ≡ canvas), preview-mask texels for preview.
    // `d.pos` / `d.bbox_target_px` / `d.inv_radius_target_px` live in
    // the same frame, so `local` is unit-coherent regardless of mode.
    out.push_str("    let target_pos = in.target_pos;\n");
    out.push_str("    let local = target_pos - d.pos;\n");
    out.push_str("    let local_dist_px = length(local);\n");
    out.push_str("    if (local_dist_px >= d.bbox_target_px) {\n");
    out.push_str("        discard;\n");
    out.push_str("    }\n");
    out.push_str("    let local_uv = local * d.inv_radius_target_px;\n");
    out.push_str("    let local_dist = length(local_uv);\n");
    out.push_str("    let theta = atan2(local_uv.y, local_uv.x);\n");
    out.push_str("    let canvas_size = vec2<f32>(\n");
    out.push_str("        f32(u.intrinsic.canvas_size.x),\n");
    out.push_str("        f32(u.intrinsic.canvas_size.y),\n");
    out.push_str("    );\n");
    match mode {
        // Stroke: target ≡ canvas, so `target_pos / canvas_size` is the
        // canvas-space normalized UV the selection texture expects.
        ShaderMode::Stroke => out.push_str(
            "    let sel = textureSampleLevel(sel_tex, sel_smp, target_pos / canvas_size, 0.0).r;\n",
        ),
        ShaderMode::Preview => out.push_str("    let sel: f32 = 1.0;\n"),
    }
    out.push_str(fs_body);
    out.push_str("}\n");

    out
}

/// Stroke-mode vertex stage — instanced quad per dab, mapped against
/// the layer's NDC viewport. Used by every compiled brush in stroke
/// mode. Includes `VsOut` + `quad_corner` since the preview vertex
/// stage uses them too and we splice exactly one of the two stages
/// into each assembled shader.
const STROKE_VERTEX_STAGE_WGSL: &str = r#"
struct VsOut {
    @builtin(position) clip:        vec4<f32>,
    @location(0) target_pos:        vec2<f32>,
    @location(1) @interpolate(flat) dab_idx: u32,
};

fn quad_corner(vi: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return corners[vi];
}

@vertex
fn vs_main(
    @builtin(vertex_index)   vi: u32,
    @builtin(instance_index) ii: u32,
) -> VsOut {
    let dab = dabs[ii];
    let corner = quad_corner(vi);
    // `dab.bbox_target_px` is the dab's bbox half-extent in the target's
    // pixel space (stroke target ≡ canvas px). The fragment stage
    // discards past the same bound, so the quad covers exactly what
    // the shader can write — no waste, no clipping. The CPU side packs
    // the same value into the dab record and uses it for the
    // layer-clip bbox, so the save-point system tracks the same
    // footprint the shader writes.
    let quad_half = dab.bbox_target_px;
    let target_pos = dab.pos + (corner * 2.0 - vec2<f32>(1.0, 1.0)) * quad_half;
    let layer_offset = u.intrinsic.layer_offset;
    let layer_size = u.intrinsic.layer_size;
    let local = target_pos - vec2<f32>(f32(layer_offset.x), f32(layer_offset.y));
    let layer_w = f32(layer_size.x);
    let layer_h = f32(layer_size.y);
    let clip = vec2<f32>(
        local.x / layer_w * 2.0 - 1.0,
        1.0 - local.y / layer_h * 2.0,
    );
    var out: VsOut;
    out.clip       = vec4<f32>(clip, 0.0, 1.0);
    out.target_pos = target_pos;
    out.dab_idx    = ii;
    return out;
}
"#;

/// Preview-mode vertex stage — single quad centred at
/// `u.intrinsic.preview_centre`, mapped against the preview mask's
/// NDC viewport (`u.intrinsic.preview_size`). The fragment shader
/// reads `dabs[0]` for the (single) record's pose; the per-fragment
/// math is unchanged from stroke mode. Repeats the `VsOut` /
/// `quad_corner` declarations so the two vertex stages are
/// drop-in alternatives — assemble_shader splices exactly one.
const PREVIEW_VERTEX_STAGE_WGSL: &str = r#"
struct VsOut {
    @builtin(position) clip:        vec4<f32>,
    @location(0) target_pos:        vec2<f32>,
    @location(1) @interpolate(flat) dab_idx: u32,
};

fn quad_corner(vi: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return corners[vi];
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let dab = dabs[0];
    let corner = quad_corner(vi);
    // Read `dab.pos` instead of `u.intrinsic.preview_centre` so the
    // dab record is the single source of truth for positioning. The
    // CPU side packs `pos = preview_centre`, making the two equivalent
    // by construction, but threading through `dab.pos` keeps the
    // vertex structurally identical to stroke's modulo the clip-space
    // mapping — the invariant is the same: target-space pos, bbox in
    // target px.
    let target_pos = dab.pos + (corner * 2.0 - vec2<f32>(1.0, 1.0)) * dab.bbox_target_px;
    let preview_size_f = vec2<f32>(
        f32(u.intrinsic.preview_size.x),
        f32(u.intrinsic.preview_size.y),
    );
    let clip = vec2<f32>(
        target_pos.x / preview_size_f.x * 2.0 - 1.0,
        1.0 - target_pos.y / preview_size_f.y * 2.0,
    );
    var out: VsOut;
    out.clip       = vec4<f32>(clip, 0.0, 1.0);
    out.target_pos = target_pos;
    out.dab_idx    = 0u;
    return out;
}
"#;

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_to_basic() {
        assert_eq!(align_to(0, 4), 0);
        assert_eq!(align_to(1, 4), 4);
        assert_eq!(align_to(4, 4), 4);
        assert_eq!(align_to(5, 4), 8);
        assert_eq!(align_to(12, 16), 16);
        assert_eq!(align_to(16, 16), 16);
    }

    #[test]
    fn struct_size_simple() {
        let fields = vec![
            DabField {
                name: "pos".into(),
                ty: WgslType::Vec2,
                pack: Arc::new(|_, _| {}),
            },
            DabField {
                name: "radius".into(),
                ty: WgslType::F32,
                pack: Arc::new(|_, _| {}),
            },
            DabField {
                name: "pad".into(),
                ty: WgslType::F32,
                pack: Arc::new(|_, _| {}),
            },
        ];
        // vec2 (8) + f32 (4) + f32 (4) = 16, aligned to 8 = 16.
        assert_eq!(compute_struct_size(&fields), 16);
    }

    #[test]
    fn struct_size_with_vec4() {
        let fields = vec![
            DabField {
                name: "a".into(),
                ty: WgslType::F32,
                pack: Arc::new(|_, _| {}),
            },
            DabField {
                name: "color".into(),
                ty: WgslType::Vec4,
                pack: Arc::new(|_, _| {}),
            },
        ];
        // f32 (4) → align to 16 (pad 12) → vec4 (16) = 32.
        assert_eq!(compute_struct_size(&fields), 32);
    }

    #[test]
    fn input_binding_emits_default_literal() {
        let b = InputBinding::Default(ScalarValue::Scalar(0.5));
        assert!(b.as_f32().starts_with("0.5"));
        assert!(b.as_vec2().starts_with("vec2<f32>(0.5"));
    }

    #[test]
    fn input_binding_passes_wired_through() {
        let b = InputBinding::Wired("d.foo".into());
        assert_eq!(b.as_f32(), "d.foo");
        assert_eq!(b.as_vec2(), "d.foo");
    }
}
