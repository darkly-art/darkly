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
//! [`crate::brush::eval::BrushNodeEvaluator::compile_wgsl`] successfully,
//! or brush load fails.
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
//!
//! ## File map
//!
//! - [`type_system`] — `WgslType`, `DabField`, `UniformField` + std430
//!   layout helpers.
//! - [`context`] — `CompileWgslCtx`, `NodeWgsl`, `InputBinding`,
//!   `ShaderMode`.
//! - [`extent`] — `ExtentContribution` / `ExtentCtx` + the per-graph
//!   composition walk.
//! - [`intrinsics`] — `IntrinsicUniforms` `repr(C)` mirror of the
//!   WGSL prelude's struct, plus its packer.
//! - [`dab_record`] — fixed-prefix intrinsic dab header + its packer.

pub mod context;
pub mod dab_record;
pub mod extent;
pub mod intrinsics;
pub mod type_system;

pub use context::{CompileWgslCtx, InputBinding, NodeWgsl, ShaderMode};
pub use dab_record::{
    intrinsic_dab_header, pack_intrinsic_dab_header, INTRINSIC_DAB_HEADER_FIELDS,
};
pub use extent::{ExtentContribution, ExtentCtx};
pub use intrinsics::{pack_intrinsic_uniforms, IntrinsicUniforms, INTRINSIC_UNIFORMS_SIZE};
pub use type_system::{DabField, DabPacker, UniformField, UniformPacker, ValuePacker, WgslType};

use std::collections::{HashMap, HashSet};

use crate::brush::eval::BrushNodeEvaluator;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{ExecutionPlan, NodeId, PortDir, PortRef};

use self::dab_record::EPS_RADIUS_TARGET_PX;
use self::extent::compose_brush_extent;
use self::type_system::{compute_struct_size, compute_struct_size_for_uniforms};

/// Below this canvas-px bbox, the dab has effectively no extent and
/// `render_compiled_preview` early-returns rather than try to compute
/// a canvas-to-target scale.
const EPS_BBOX_CANVAS_PX: f32 = 1e-3;

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
    /// includes the intrinsic header fields ([`INTRINSIC_DAB_HEADER_FIELDS`])
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
    /// every node's [`ExtentContribution`] over the graph. The
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
    evaluators: &HashMap<String, Box<dyn BrushNodeEvaluator>>,
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

// ── Helpers ─────────────────────────────────────────────────────────────

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

// ── Shader assembly ─────────────────────────────────────────────────────

fn assemble_shader(
    mode: ShaderMode,
    dab_fields: &[DabField],
    uniform_fields: &[UniformField],
    node_decls: &str,
    fs_body: &str,
    terminal_bindings: &str,
) -> String {
    let mut out = String::new();
    out.push_str(include_str!("../../../../../shaders/brush/_shape.wgsl"));
    out.push('\n');
    out.push_str(include_str!("../../../../../shaders/brush/_prelude.wgsl"));
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
