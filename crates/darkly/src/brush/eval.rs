//! Brush graph evaluation runtime.
//!
//! The runner takes a compiled `ExecutionPlan`, a table of evaluator
//! closures, and a flat `Vec<Option<ScalarValue>>` slot table.  Per-dab
//! evaluation is zero-heap-allocation: the slot table is pre-sized and
//! reused across dabs.

use std::collections::HashMap;
use std::sync::Arc;

use crate::gpu::params::ParamValue;
use crate::nodegraph::{
    ExecStep, ExecutionPlan, Graph, InputSlot, NodeId, NodeRegistration, PortDef, PortDir, PortRef,
};

use super::curve_math::CurveLut;
use super::nodes::{paint_color, pen_input};

use super::gpu_context::BrushGpuContext;
use super::wgsl::CompiledBrush;
use super::wire::{BrushWireType, ScalarValue};

// ── Evaluator trait ─────────────────────────────────────────────────

/// Context passed to each node's CPU evaluator.
///
/// Connected input values arrive as two parallel slices: port name +
/// metadata in `input_slots`, post-remap value in `input_values`.
/// [`Self::input`] linearly scans `input_slots` — per-node input count
/// is small (typically 1–3, never more than ~8), so a `HashMap` would
/// only trade a per-step heap allocation for no real speedup.
pub struct EvalContext<'a> {
    /// Connected input ports for this step, in the same order as
    /// `input_values`. Disconnected ports are absent — [`Self::input`]
    /// falls back to the port's default for those.
    pub input_slots: &'a [InputSlot],
    /// Per-port post-remap value, parallel to `input_slots`. `None` when
    /// the upstream slot held no value (the named input then resolves to
    /// the port default, same as if it were disconnected).
    pub input_values: &'a [Option<ScalarValue>],
    /// Per-instance parameter overrides from the graph.
    pub params: &'a [ParamValue],
    /// Port definitions for this node instance (for reading defaults).
    pub port_defs: &'a [PortDef<BrushWireType>],
    /// Precomputed curve LUT, if this node has a Curve parameter.
    pub lut: Option<&'a CurveLut>,
    /// PRNG seed for this stroke (deterministic per-stroke randomness).
    pub stroke_seed: u32,
    /// Index of the current dab within the stroke (0-based).
    pub dab_index: u32,
    /// This node instance's ID (used to salt PRNG for independence).
    pub node_id: NodeId,
}

impl EvalContext<'_> {
    /// Read an input value, falling back to the port's default if disconnected.
    pub fn input(&self, name: &str) -> ScalarValue {
        for (i, slot) in self.input_slots.iter().enumerate() {
            if slot.port_name == name {
                if let Some(val) = self.input_values[i] {
                    return val;
                }
                break;
            }
        }
        // Fall back to port default.
        for port in self.port_defs {
            if port.name == name && port.dir == PortDir::Input {
                return ScalarValue::Scalar(port.default);
            }
        }
        ScalarValue::default()
    }

    /// Read an input as f32 (with coercion and default fallback).
    pub fn input_f32(&self, name: &str) -> f32 {
        self.input(name).as_f32()
    }

    /// Read a parameter by index as f32.
    pub fn param_f32(&self, index: usize) -> f32 {
        match self.params.get(index) {
            Some(ParamValue::Float(v)) => *v,
            Some(ParamValue::Int(v)) => *v as f32,
            _ => 0.0,
        }
    }

    /// Read a parameter by index as &str.
    pub fn param_str(&self, index: usize) -> &str {
        match self.params.get(index) {
            Some(ParamValue::String(s)) => s.as_str(),
            _ => "",
        }
    }

    /// Read a parameter by index as curve control points.
    pub fn param_curve(&self, index: usize) -> &[[f32; 2]] {
        match self.params.get(index) {
            Some(ParamValue::Curve(pts)) => pts.as_slice(),
            _ => &[[0.0, 0.0], [1.0, 1.0]],
        }
    }

    /// O(1) curve lookup using the precomputed LUT.
    /// Falls back to identity (returns `t` unchanged) if no LUT is cached.
    #[inline]
    pub fn curve_lookup(&self, t: f32) -> f32 {
        match self.lut {
            Some(lut) => lut.evaluate(t),
            None => t,
        }
    }

    /// Deterministic pseudo-random scalar in `[0, 1)` keyed by `index`.
    /// The PRNG is salted with this node's ID so multiple random-using
    /// nodes in the same graph yield independent streams; the stroke
    /// seed makes runs reproducible for replays and checkpoint restores.
    ///
    /// Callers pulling multiple independent values on a single dab
    /// encode that into `index` — e.g. scatter uses `dab_index * 2` and
    /// `dab_index * 2 + 1` for its x and y offsets.
    #[inline]
    pub fn prng_at(&self, index: u32) -> f32 {
        let salt = self.node_id.0 as u32;
        let seed = self.stroke_seed.wrapping_add(salt.wrapping_mul(0x9E3779B9));
        prng_f32(seed, index)
    }
}

/// Gather a step's connected inputs from the slot table into `scratch`,
/// applying wire-boundary range remap. The output is parallel to
/// `input_slots` — index `i` holds the resolved value for `input_slots[i]`,
/// or `None` when the upstream slot hasn't been written. `scratch` is the
/// runner's reused buffer; its allocation amortises across dabs.
///
/// This is where the "everything speaks 0-1" intent in
/// [`crate::brush::wire`] actually lives: when both ends of a wire
/// declare a `natural_range`, the value gets affinely remapped at the
/// boundary; otherwise it passes through raw (preserving math-node and
/// over-drag-slider passthrough).
fn gather_inputs_into(
    slots: &[Option<ScalarValue>],
    input_slots: &[InputSlot],
    dest_node: NodeId,
    node_data: &HashMap<NodeId, NodeData>,
    scratch: &mut Vec<Option<ScalarValue>>,
) {
    scratch.clear();
    scratch.reserve(input_slots.len());
    for slot_info in input_slots {
        let entry = slots[slot_info.slot].map(|val| {
            remap_for_wire(
                val,
                &slot_info.source,
                dest_node,
                &slot_info.port_name,
                node_data,
            )
        });
        scratch.push(entry);
    }
}

/// Apply the wire-boundary remap if both ends of the wire declare a
/// `natural_range`. Operates on the scalar-coercible variants
/// (`Scalar`/`Int`/`Bool`); everything else (textures, colors, vectors)
/// passes through unchanged.
fn remap_for_wire(
    value: ScalarValue,
    source: &PortRef,
    dest_node: NodeId,
    dest_port: &str,
    node_data: &HashMap<NodeId, NodeData>,
) -> ScalarValue {
    let src_range = node_data
        .get(&source.node)
        .and_then(|n| {
            n.port_defs
                .iter()
                .find(|p| p.name == source.port && p.dir == PortDir::Output)
        })
        .and_then(|p| p.natural_range);
    let dst_range = node_data
        .get(&dest_node)
        .and_then(|n| {
            n.port_defs
                .iter()
                .find(|p| p.name == dest_port && p.dir == PortDir::Input)
        })
        .and_then(|p| p.natural_range);
    let (Some(src), Some(dst)) = (src_range, dst_range) else {
        return value;
    };
    match value {
        ScalarValue::Scalar(_) | ScalarValue::Int(_) | ScalarValue::Bool(_) => {
            ScalarValue::Scalar(remap_scalar(value.as_f32(), src, dst))
        }
        _ => value,
    }
}

/// Affine remap from `src` range to `dst` range. Not clamped — consistent
/// with the "ranges are UI hints, not enforced" contract; consumers that
/// need a hard bound clamp inside their own evaluator.
#[inline]
fn remap_scalar(value: f32, src: (f32, f32), dst: (f32, f32)) -> f32 {
    let (src_min, src_max) = src;
    let (dst_min, dst_max) = dst;
    let denom = src_max - src_min;
    // Degenerate source range — collapse to dst_min rather than divide by zero.
    if denom == 0.0 {
        return dst_min;
    }
    let fraction = (value - src_min) / denom;
    dst_min + fraction * (dst_max - dst_min)
}

/// Apply the framework-managed stroke prologue declared on a node's
/// registration. Called from [`BrushGraphRunner::begin_stroke`] before
/// the per-node `begin_stroke` hook runs. Centralising this here means
/// adding a new terminal can never silently drift away from the
/// established lifecycles — the four-way copy-paste that used to live
/// in `paint`/`watercolor`/`smudge`/`liquify` collapses into one
/// declaration plus the enum dispatch below.
fn apply_lifecycle(lifecycle: super::node::Lifecycle, gpu: &mut BrushGpuContext) {
    use super::node::Lifecycle;
    match lifecycle {
        Lifecycle::None => {}
        Lifecycle::ClearScratchToTransparent => {
            if let Some(scratch) = gpu.scratch.as_deref() {
                scratch.clear_to_transparent(&mut gpu.encoder);
            }
        }
        Lifecycle::SeedScratchFromPreStroke => {
            let Some(pre_stroke) = gpu.pre_stroke_texture else {
                return;
            };
            let Some(scratch) = gpu.scratch.as_deref() else {
                return;
            };
            scratch.seed_from_pre_stroke(&mut gpu.encoder, pre_stroke);
        }
    }
}

/// Deterministic PRNG: hash seed + index to produce a 0-1 float.
/// xorshift-style for speed; shared by all nodes via `EvalContext::prng_at`.
#[inline]
fn prng_f32(seed: u32, index: u32) -> f32 {
    let mut h = seed.wrapping_add(index.wrapping_mul(2654435761));
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    (h & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

/// Canvas-space positioning info read from the graph's preview terminal
/// after a preview-mode evaluation. Consumed by the overlay to place the
/// `KIND_MASKED_STAMP` primitive — the mask texture itself is bound to
/// the overlay separately.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BrushPreviewInfo {
    /// Half-extent in canvas pixels — the overlay primitive's `p1`.
    pub half_extent_canvas_px: [f32; 2],
    /// Rotation in radians — the overlay primitive's `rotation`.
    pub rotation_rad: f32,
}

/// Trait implemented by each node to produce output values.
pub trait BrushNodeEvaluator: Send + Sync {
    /// Evaluate the node on the CPU and return named output values.
    ///
    /// Called once per dab for each CPU node in topological order.
    /// GPU nodes return empty from this — they use `evaluate_gpu` instead.
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)>;

    /// Evaluate the node on the GPU, recording render passes into the
    /// encoder.  Returns named output values (e.g. texture handles).
    ///
    /// Default implementation is a no-op — CPU-only nodes don't override
    /// this.  GPU nodes (`is_gpu: true`) override this and ignore
    /// `evaluate_cpu`.
    fn evaluate_gpu(
        &self,
        _ctx: &EvalContext,
        _gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// Preview-mode GPU evaluation. Called by the engine during hover
    /// preview regen, in place of `evaluate_gpu`. Default delegates so
    /// most nodes don't differentiate; nodes that bake per-dab deposition
    /// (stamp's flow, color, scatter) override this to produce a
    /// preview-appropriate output (typically a clean rotated/aspect-baked
    /// shape texture sized to the brush's canvas-pixel extent).
    ///
    /// Terminal nodes that own preview rendering (e.g. `color_output`)
    /// also override this — they read a `brush_preview` input texture and
    /// blit it into `gpu.preview_mask_view`, then publish placement info
    /// via `gpu.brush_preview_info`.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        self.evaluate_gpu(ctx, gpu)
    }

    /// Stroke-scoped setup. Called at stroke start and on every rewind
    /// boundary (full or partial) — whenever the scratch must be reset
    /// to this terminal's starting state. Non-terminal nodes default to
    /// no-op.
    ///
    /// The `EvalContext`'s slot-driven inputs are *not* seeded for this
    /// hook — it runs before any dab, so `ctx.input` will only return
    /// port defaults. That's deliberate: the hook's job is lifecycle,
    /// not per-dab sampling.
    fn begin_stroke(&self, _ctx: &EvalContext, _gpu: &mut BrushGpuContext) {}

    /// Per-pen-event commit. Called after every event's dabs have rendered
    /// into the scratch. The terminal pushes the scratch onto the layer
    /// however its semantics require. Non-terminal nodes default to no-op.
    ///
    /// Inputs reflect the LAST dab's evaluated slot values — `commit`
    /// runs after `execute_gpu`, so the slot table still holds the most
    /// recent dab's results. Ports declared as "applied at commit"
    /// (e.g. `paint.opacity`) read their wired value through
    /// `ctx.input_*`. `begin_stroke` is different: it runs before any
    /// dab and sees port defaults only.
    fn commit(&self, _ctx: &EvalContext, _gpu: &mut BrushGpuContext) {}

    /// Flush any per-rendering-phase work the terminal has queued during
    /// the preceding `evaluate_gpu` calls. Called at the end of every
    /// dab-rendering phase (`render_from_stabilized_range_to`,
    /// `render_from_stabilized_tail`) just before that phase's
    /// `submit_final`.
    ///
    /// Dab-batching terminals (paint, watercolor_batched) use this to dispatch their
    /// batched work; fragment-path terminals that already record per-dab
    /// passes inline keep the default no-op.
    fn flush_dabs(&self, _ctx: &EvalContext, _gpu: &mut BrushGpuContext) {}

    /// Emit this node's contribution to a compiled WGSL fragment
    /// shader. Used only by brushes that terminate in `paint`
    /// (the compiled execution path); brushes on the per-dab dispatch
    /// path never call this. Returning `Err` makes the whole brush
    /// fail to load when its terminal asks for compilation — there is
    /// no runtime fallback. See [`crate::brush::wgsl`].
    fn compile_wgsl(
        &self,
        _cctx: &crate::brush::wgsl::CompileWgslCtx,
    ) -> Result<crate::brush::wgsl::NodeWgsl, String> {
        Err("node has no WGSL implementation".into())
    }

    /// Preview-mode replacement for the terminal's `compile_wgsl`
    /// body. Default delegates to `compile_wgsl`, which is correct for
    /// terminals whose stroke body doesn't reference `@group(2)` /
    /// `@group(3)` bindings — the preview skeleton substitutes `sel =
    /// 1.0` and omits both groups, so the same body works under both
    /// modes.
    ///
    /// Terminals that sample scratch / atlas in their stroke body
    /// (watercolor's pickup atlas, smudge / liquify's `scratch_mirror`)
    /// override this to emit a body that doesn't need those bindings —
    /// typically a neutral-color mask of the brush footprint. Only the
    /// `body` field of the returned `NodeWgsl` is consumed; decls /
    /// dab_fields / uniform_fields / outputs come from the stroke pass
    /// and are shared across both shader variants (helper functions a
    /// preview body references — e.g. liquify's `falloff_fn` — live in
    /// `decls` and are visible to both skeletons).
    ///
    /// Non-terminal nodes never have this called — only nodes for
    /// which `is_terminal()` returns `true` invoke the hook.
    fn compile_preview_body(
        &self,
        cctx: &crate::brush::wgsl::CompileWgslCtx,
    ) -> Result<crate::brush::wgsl::NodeWgsl, String> {
        self.compile_wgsl(cctx)
    }

    /// Per-node contribution to the brush's dab bounding-box extent.
    /// Composed by the framework at brush-compile time into a single
    /// `(factor, extra_px)` pair on [`crate::brush::wgsl::CompiledBrush`];
    /// the `paint` terminal uses it to size both the per-dab
    /// rasterized quad (via `dab.bbox_target_px`) and the CPU
    /// layer-clip bbox, ensuring the save-point system tracks exactly
    /// what the shader writes.
    ///
    /// Default `Identity` — only nodes that change the dab footprint
    /// (shape masks, displacement / warp) override. See
    /// [`crate::brush::wgsl::ExtentContribution`].
    fn extent(
        &self,
        _ctx: &crate::brush::wgsl::ExtentCtx,
    ) -> crate::brush::wgsl::ExtentContribution {
        crate::brush::wgsl::ExtentContribution::Identity
    }
}

// ── Graph runner ────────────────────────────────────────────────────

/// A compiled, ready-to-run brush graph with pre-allocated slot table.
///
/// The evaluation model is **compile once, evaluate per-dab**.  When the
/// user edits the brush graph, we compile a new runner (cheap — just a
/// topo sort and slot allocation).  During a stroke, each dab reuses the
/// same runner with zero heap allocation:
///
/// 1. `seed_sensors()` — writes tablet data directly into pre-known slot
///    indices (no virtual dispatch, no HashMap lookup on the hot path).
/// 2. `execute_cpu()` — walks the topologically-sorted plan, calling each
///    CPU node's evaluator which reads inputs from and writes outputs to
///    the flat slot table.
/// 3. `execute_gpu()` — walks GPU nodes in topological order, calling
///    `evaluate_gpu()` which records render passes and writes texture
///    handles back to the slot table.
///
/// The slot table is a flat `Vec<Option<ScalarValue>>` — one entry per
/// output port in the graph, indexed by the compiler-assigned slot number.
/// This avoids per-node HashMaps and keeps evaluation cache-friendly.
pub struct BrushGraphRunner {
    /// Topologically-sorted execution steps with pre-assigned slot indices.
    /// Compiled once from the graph; determines evaluation order and which
    /// slot each port reads from / writes to.
    plan: ExecutionPlan,
    /// Per-step evaluator pointer, resolved once at runner build by looking
    /// up `step.type_id` in the supplied evaluator map. `None` for steps
    /// whose type is unregistered (treated as a no-op during eval — matches
    /// the prior HashMap-lookup-fail behaviour without re-introducing the
    /// per-dab lookup).
    step_evaluators: Vec<Option<Arc<dyn BrushNodeEvaluator>>>,
    /// Flat slot table indexed by compiler-assigned slot number.  Pre-sized
    /// to `plan.slot_count` and reused across dabs — `clear_slots()` resets
    /// it between evaluations without reallocating.
    slots: Vec<Option<ScalarValue>>,
    /// Reusable per-step input scratch — parallel to the current step's
    /// `input_slots`. Cleared and refilled by `gather_inputs_into` before
    /// each evaluator call. Living on the runner amortises the allocation
    /// across all dabs of a stroke (it grows once to the max input count
    /// in the graph, then never reallocates).
    inputs_scratch: Vec<Option<ScalarValue>>,
    /// Cached per-node params and port defs, copied from the graph at
    /// compile time so we don't need to borrow the graph during evaluation.
    node_data: HashMap<NodeId, NodeData>,
    /// Pre-resolved slot indices for pen_input's output ports.  Stored
    /// separately so `seed_sensors()` can write directly without walking
    /// the plan or doing any lookups — this is the hottest path (called
    /// once per dab, potentially hundreds of times per stroke).
    pen_input_slots: Vec<(String, usize)>,
    /// Pre-resolved slot index for paint_color's output.  Same rationale
    /// as `pen_input_slots` — avoid plan traversal on the hot path.
    paint_color_slot: Option<usize>,
    /// Pre-resolved slot index of the terminal `dab_size` output, used
    /// by the stroke engine to size spacing and save-point bboxes.
    /// Resolved by walking `plan.steps` once for any step where
    /// `is_terminal == true` and the output port `dab_size` exists.
    /// Replaces a per-dab string-match loop against the hard-coded
    /// list `["paint", "watercolor", "smudge", "liquify"]`, which
    /// silently ignored any future terminal that publishes the same
    /// port.
    dab_size_slot: Option<usize>,
    /// PRNG seed for the current stroke, set by `seed_sensors()`.
    stroke_seed: u32,
    /// Index of the current dab, set by `seed_sensors()`.
    dab_index: u32,
    /// Compiled WGSL for this brush, populated by `compile_graph` when
    /// the graph terminates in `paint`. `None` for per-dab
    /// dispatch brushes. The runner copies this into the
    /// `BrushGpuContext` at `dispatch_gpu` time so the terminal can
    /// read its dab/uniform layouts.
    compiled: Option<Arc<CompiledBrush>>,
}

struct NodeData {
    params: Vec<ParamValue>,
    port_defs: Vec<PortDef<BrushWireType>>,
    lut: Option<CurveLut>,
}

fn build_eval_ctx<'a>(
    step: &'a ExecStep,
    input_slots: &'a [InputSlot],
    input_values: &'a [Option<ScalarValue>],
    node_data: &'a HashMap<NodeId, NodeData>,
    stroke_seed: u32,
    dab_index: u32,
) -> EvalContext<'a> {
    let node = node_data.get(&step.node_id);
    EvalContext {
        input_slots,
        input_values,
        params: node.map(|n| n.params.as_slice()).unwrap_or(&[]),
        port_defs: node.map(|n| n.port_defs.as_slice()).unwrap_or(&[]),
        lut: node.and_then(|n| n.lut.as_ref()),
        stroke_seed,
        dab_index,
        node_id: step.node_id,
    }
}

impl BrushGraphRunner {
    /// Build a runner from a graph and a registry of evaluators.
    ///
    /// The evaluator map is consumed: every entry is converted from
    /// `Box<dyn ...>` to `Arc<dyn ...>` (a refcount-only transfer of the
    /// existing heap pointer — no extra allocation), and one `Arc` is
    /// stored per `ExecStep` so the per-dab path can index by step
    /// position without a `HashMap` lookup.
    pub fn new(
        graph: &Graph<BrushWireType>,
        registry: &HashMap<String, NodeRegistration<BrushWireType>>,
        evaluators: HashMap<String, Box<dyn BrushNodeEvaluator>>,
    ) -> Result<Self, crate::nodegraph::GraphError> {
        let plan = crate::nodegraph::compile(graph, registry)?;
        let slots = vec![None; plan.slot_count];

        // Convert the owning `Box<dyn ...>` map to `Arc<dyn ...>` once
        // so steps that share a node type share one evaluator instance.
        // `Arc::from(Box<T>)` reuses the existing heap allocation, so
        // this isn't a fresh allocation per evaluator.
        let evaluators: HashMap<String, Arc<dyn BrushNodeEvaluator>> = evaluators
            .into_iter()
            .map(|(k, v)| (k, Arc::from(v)))
            .collect();

        // Resolve each step's evaluator once at runner-build time. The
        // hot path then indexes by step position; no per-dab string
        // lookup, no Box clone.
        let step_evaluators: Vec<Option<Arc<dyn BrushNodeEvaluator>>> = plan
            .steps
            .iter()
            .map(|step| evaluators.get(&step.type_id).cloned())
            .collect();

        // Cache per-node instance data for fast access during eval.
        // Precompute curve LUTs for nodes with Curve parameters.
        let mut node_data = HashMap::new();
        for step in &plan.steps {
            if let Some(node) = graph.nodes.get(&step.node_id) {
                let lut = node.params.iter().find_map(|p| match p {
                    ParamValue::Curve(pts) if pts.len() >= 2 => Some(CurveLut::from_points(pts)),
                    _ => None,
                });
                node_data.insert(
                    step.node_id,
                    NodeData {
                        params: node.params.clone(),
                        port_defs: node.ports.clone(),
                        lut,
                    },
                );
            }
        }

        // Find pen_input node's output slots for direct seeding.
        let pen_input_slots = plan
            .steps
            .iter()
            .find(|s| s.type_id == pen_input::TYPE_ID)
            .map(|s| s.output_slots.clone())
            .unwrap_or_default();

        // Find paint_color node's color output slot.
        let paint_color_slot = plan
            .steps
            .iter()
            .find(|s| s.type_id == paint_color::TYPE_ID)
            .and_then(|s| s.output_slots.iter().find(|(name, _)| name == "color"))
            .map(|(_, slot)| *slot);

        // Find the terminal's `dab_size` output slot. Whichever
        // terminal the graph uses owns the spacing unit; the first
        // terminal in plan order wins (every built-in terminal is
        // single-output for `dab_size`).
        let dab_size_slot = plan.steps.iter().filter(|s| s.is_terminal).find_map(|s| {
            s.output_slots
                .iter()
                .find(|(name, _)| name == "dab_size")
                .map(|(_, slot)| *slot)
        });

        // Prime the input scratch with the largest input-slot count any
        // step in the graph needs, so `gather_inputs_into`'s reserve()
        // never causes a reallocation during a stroke.
        let max_inputs = plan
            .steps
            .iter()
            .map(|s| s.input_slots.len())
            .max()
            .unwrap_or(0);

        Ok(Self {
            plan,
            step_evaluators,
            slots,
            inputs_scratch: Vec::with_capacity(max_inputs),
            node_data,
            pen_input_slots,
            paint_color_slot,
            dab_size_slot,
            stroke_seed: 0,
            dab_index: 0,
            compiled: None,
        })
    }

    /// Attach a pre-built [`CompiledBrush`] to this runner. Called by
    /// [`crate::brush::compile_graph`] when the graph terminates in
    /// `paint`. Idempotent — overwrites any prior value.
    pub fn set_compiled_brush(&mut self, compiled: Arc<CompiledBrush>) {
        self.compiled = Some(compiled);
    }

    /// Returns the compiled WGSL for this brush, if the graph
    /// terminates in `paint`.
    pub fn compiled_brush(&self) -> Option<Arc<CompiledBrush>> {
        self.compiled.clone()
    }

    /// Returns `true` if the graph terminates in a compiled-WGSL
    /// terminal (any node whose registration sets `is_terminal: true`).
    /// Type-owned dispatch: no central list of terminal type_ids — the
    /// compiler stamps the flag onto every [`ExecStep`] from the
    /// registry. Used by [`crate::brush::compile_graph`] to decide
    /// whether to run the WGSL compile step.
    pub fn has_terminal(&self) -> bool {
        self.plan.steps.iter().any(|step| step.is_terminal)
    }

    /// Build a name → value map of every output slot in the graph,
    /// keyed by `n{node_id}_{port_name}` (matching the convention
    /// [`crate::brush::wgsl::CompileWgslCtx::dab_field_name`]
    /// uses). Called by `dispatch_gpu` for compiled brushes; the
    /// compiled terminal reads from this to pack per-dab records and
    /// uniforms. Returns an empty map for graphs with no slots.
    fn build_slot_outputs(&self) -> HashMap<String, ScalarValue> {
        let mut out = HashMap::with_capacity(self.slots.len());
        for step in &self.plan.steps {
            for (port_name, slot_idx) in &step.output_slots {
                if let Some(val) = self.slots[*slot_idx] {
                    out.insert(format!("n{}_{}", step.node_id.0, port_name), val);
                }
            }
        }
        out
    }

    /// Seed sensor output slots directly from pen data.
    ///
    /// This is the hot path — no virtual dispatch, just memcpy into
    /// pre-known slot indices.  `stroke_seed` and `dab_index` are stored
    /// for random nodes to read during evaluation.
    pub fn seed_sensors(
        &mut self,
        info: &super::paint_info::PaintInformation,
        color: [f32; 4],
        stroke_seed: u32,
        dab_index: u32,
    ) {
        self.stroke_seed = stroke_seed;
        self.dab_index = dab_index;

        for (name, slot) in &self.pen_input_slots {
            let value = match name.as_str() {
                "pressure" => ScalarValue::Scalar(info.pressure),
                "x_tilt" => ScalarValue::Scalar(info.x_tilt),
                "y_tilt" => ScalarValue::Scalar(info.y_tilt),
                "tilt_magnitude" => ScalarValue::Scalar(info.tilt_magnitude),
                "tilt_direction" => ScalarValue::Scalar(info.tilt_direction),
                "rotation" => ScalarValue::Scalar(info.rotation),
                "tangential_pressure" => ScalarValue::Scalar(info.tangential_pressure),
                "speed" => ScalarValue::Scalar(info.speed),
                "distance" => ScalarValue::Scalar(info.distance),
                "drawing_angle" => ScalarValue::Scalar(info.drawing_angle),
                "time" => ScalarValue::Scalar(info.time),
                "position" => ScalarValue::Vec2(info.pos),
                "motion" => ScalarValue::Vec2(info.motion),
                "index" => ScalarValue::Int(info.index as i32),
                "fade" => ScalarValue::Scalar(info.fade),
                _ => continue,
            };
            self.slots[*slot] = Some(value);
        }

        // Seed paint_color if present.
        if let Some(slot) = self.paint_color_slot {
            self.slots[slot] = Some(ScalarValue::Color(color));
        }
    }

    /// Execute all CPU nodes in topological order.
    ///
    /// Call `seed_sensors()` first.  After this returns, output slots
    /// contain the final values for this dab.
    pub fn execute_cpu(&mut self) {
        let n = self.plan.steps.len();
        for idx in 0..n {
            // Field accesses below all go through `self.<field>` directly so
            // Rust's disjoint-field borrow rules split them: `self.plan`,
            // `self.step_evaluators`, `self.slots`, `self.node_data`, and
            // `self.inputs_scratch` are borrowed independently.
            let step = &self.plan.steps[idx];
            if step.type_id == pen_input::TYPE_ID
                || step.type_id == paint_color::TYPE_ID
                || step.is_gpu
            {
                continue;
            }

            let Some(evaluator) = self.step_evaluators[idx].clone() else {
                continue;
            };

            // Gather connected inputs from the slot table into the
            // runner's reused scratch buffer, applying wire-boundary
            // range remap where both source and dest ports declare a
            // `natural_range`. Zero per-dab heap allocs on the steady
            // state — the scratch grows once during the first stroke.
            gather_inputs_into(
                &self.slots,
                &step.input_slots,
                step.node_id,
                &self.node_data,
                &mut self.inputs_scratch,
            );

            let ctx = build_eval_ctx(
                step,
                &step.input_slots,
                &self.inputs_scratch,
                &self.node_data,
                self.stroke_seed,
                self.dab_index,
            );

            let outputs = evaluator.evaluate_cpu(&ctx);

            // Write outputs to their assigned slots. `ctx`'s borrows on
            // `self.inputs_scratch` (immut) and `step.input_slots` (immut)
            // are disjoint from `self.slots`, so the slot write below
            // doesn't conflict with `ctx` still being in scope.
            for (port_name, value) in outputs {
                for (name, slot_idx) in &step.output_slots {
                    if *name == port_name {
                        self.slots[*slot_idx] = Some(value);
                        break;
                    }
                }
            }
        }
    }

    /// Execute all GPU nodes in topological order.
    ///
    /// Call `seed_sensors()` and `execute_cpu()` first — GPU nodes read
    /// their scalar inputs (size, opacity, color, position) from the slot
    /// table populated by CPU nodes.  GPU nodes record render passes into
    /// the encoder and write texture handles back to the slot table.
    pub fn execute_gpu(&mut self, gpu: &mut BrushGpuContext) {
        self.dispatch_gpu(gpu, |ev, ctx, gpu| ev.evaluate_gpu(ctx, gpu));
    }

    /// Walk GPU steps invoking each evaluator's `render_preview` hook
    /// instead of `evaluate_gpu`. Same slot-table plumbing — non-terminals
    /// produce shape-appropriate outputs (e.g. stamp emits a B&W tip
    /// texture sized to the brush's canvas-pixel extent), terminals
    /// consume them and render into the overlay's preview mask, publishing
    /// placement info via `gpu.brush_preview_info`.
    pub fn render_preview_pipeline(&mut self, gpu: &mut BrushGpuContext) {
        self.dispatch_gpu(gpu, |ev, ctx, gpu| ev.render_preview(ctx, gpu));
    }

    /// Shared walker for the per-dab GPU pipeline. Wires inputs from the
    /// slot table, runs the evaluator-supplied closure (`evaluate_gpu` for
    /// stroke/dab evaluation, `render_preview` for preview regen), and
    /// writes the resulting outputs back to their slots.
    fn dispatch_gpu<F>(&mut self, gpu: &mut BrushGpuContext, mut f: F)
    where
        F: FnMut(
            &dyn BrushNodeEvaluator,
            &EvalContext,
            &mut BrushGpuContext,
        ) -> Vec<(String, ScalarValue)>,
    {
        // Attach the brush's compiled WGSL and a snapshot of every
        // output slot keyed by `n{id}_{port}`. The terminal reads
        // these to pack per-dab records and uniforms.
        let is_compiled = self.compiled.is_some();
        if let Some(compiled) = &self.compiled {
            gpu.compiled_brush = Some(compiled.clone());
            gpu.slot_outputs_owned = Some(self.build_slot_outputs());
        }

        let n = self.plan.steps.len();
        for idx in 0..n {
            let step = &self.plan.steps[idx];
            if !step.is_gpu {
                continue;
            }
            // Every upstream GPU node's contribution is fused into
            // the terminal's fragment shader, so only the terminal
            // step needs its `evaluate_gpu` invoked per dab — that's
            // where the per-dab record gets queued. Skipping the
            // others is the load-bearing perf win.
            let Some(evaluator) = self.step_evaluators[idx].clone() else {
                continue;
            };
            if is_compiled && !step.is_terminal {
                continue;
            }

            // Gather connected inputs from the slot table into the
            // runner's reused scratch buffer, applying wire-boundary
            // range remap where both source and dest ports declare a
            // `natural_range`.
            gather_inputs_into(
                &self.slots,
                &step.input_slots,
                step.node_id,
                &self.node_data,
                &mut self.inputs_scratch,
            );

            let ctx = build_eval_ctx(
                step,
                &step.input_slots,
                &self.inputs_scratch,
                &self.node_data,
                self.stroke_seed,
                self.dab_index,
            );

            // Pure-math nodes promoted to the GPU phase (because an input
            // depends on a GPU output) only implement `evaluate_cpu`. Run
            // it here so the slot table fills in topological order; the
            // `evaluate_gpu` closure runs too and no-ops (empty default).
            // Declared-GPU nodes take the opposite path: `evaluate_cpu`
            // returns empty, `evaluate_gpu` does the work.
            let mut outputs = evaluator.evaluate_cpu(&ctx);
            let gpu_outputs = f(evaluator.as_ref(), &ctx, gpu);

            outputs.extend(gpu_outputs);

            // Write outputs to their assigned slots. Linear scan with
            // string compare per produced output × per step × per dab.
            for (port_name, value) in outputs {
                for (name, slot_idx) in &step.output_slots {
                    if *name == port_name {
                        self.slots[*slot_idx] = Some(value);
                        break;
                    }
                }
            }
        }
    }

    /// Run the framework-managed stroke prologue, then dispatch
    /// `begin_stroke` to every GPU node's evaluator in topological order.
    /// Runs once per stroke-start and once per rewind boundary, before
    /// any dab.
    ///
    /// The prologue is driven by the [`crate::brush::node::Lifecycle`]
    /// each node's registration declares — clearing the scratch to
    /// transparent (paint, watercolor) or seeding it from the
    /// pre-stroke snapshot (smudge, liquify). This lives here, not in
    /// each terminal's `begin_stroke`, so adding a new terminal can't
    /// silently drift from the established lifecycles. The pending-dab
    /// queue is also reset here for the same reason — every terminal
    /// needs it cleared at stroke-start; no point copy-pasting that
    /// line per terminal.
    pub fn begin_stroke(&mut self, gpu: &mut BrushGpuContext) {
        gpu.clear_pending_dabs();
        let registry = crate::brush::registry();
        self.dispatch_lifecycle(gpu, false, |type_id, ev, ctx, gpu| {
            let lifecycle = registry
                .get(type_id)
                .map(|r| r.lifecycle)
                .unwrap_or(super::node::Lifecycle::None);
            apply_lifecycle(lifecycle, gpu);
            ev.begin_stroke(ctx, gpu);
        });
    }

    /// Dispatch `commit` to every GPU node's evaluator in topological
    /// order. Runs once per pen event after that event's dabs have
    /// finished compositing into the scratch.
    pub fn commit(&mut self, gpu: &mut BrushGpuContext) {
        // Gather inputs from the slot table so terminals that read
        // ports at commit time (e.g. `paint.opacity` wired
        // to `pen.pressure` for the Airbrush) see the actual wired
        // value, not the port default.
        self.dispatch_lifecycle(gpu, true, |_id, ev, ctx, gpu| ev.commit(ctx, gpu));
    }

    /// Dispatch `flush_dabs` to every GPU node's evaluator in
    /// topological order. Called at the end of each dab-rendering phase
    /// (segments / tail) so compute-path terminals can issue their batched
    /// dispatch before the phase's `submit_final`. Fragment-path
    /// terminals no-op.
    pub fn flush_dabs(&mut self, gpu: &mut BrushGpuContext) {
        self.dispatch_lifecycle(gpu, false, |_id, ev, ctx, gpu| ev.flush_dabs(ctx, gpu));
    }

    /// Shared walker for lifecycle hooks. When `gather_from_slots` is
    /// true, each step's inputs are pulled from the live slot table —
    /// the same plumbing `dispatch_gpu` uses — so commits see the latest
    /// per-dab wire values. When false, inputs are empty and evaluators
    /// fall back to port defaults (correct for `begin_stroke`, which
    /// runs before any dab populates the table).
    ///
    /// The closure receives the step's `type_id` so framework-managed
    /// hooks (e.g. the begin-stroke lifecycle) can look up registration
    /// metadata without each callsite re-fetching the step.
    fn dispatch_lifecycle<F>(
        &mut self,
        gpu: &mut BrushGpuContext,
        gather_from_slots: bool,
        mut f: F,
    ) where
        F: FnMut(&str, &dyn BrushNodeEvaluator, &EvalContext, &mut BrushGpuContext),
    {
        let n = self.plan.steps.len();
        for idx in 0..n {
            let step = &self.plan.steps[idx];
            if !step.is_gpu {
                continue;
            }
            let Some(evaluator) = self.step_evaluators[idx].clone() else {
                continue;
            };
            // `gather_from_slots = false` keeps the input slices empty —
            // `EvalContext::input` then falls through to port defaults,
            // matching the prior `HashMap::new()` semantics.
            let (input_slots_view, input_values_view): (&[InputSlot], &[Option<ScalarValue>]) =
                if gather_from_slots {
                    gather_inputs_into(
                        &self.slots,
                        &step.input_slots,
                        step.node_id,
                        &self.node_data,
                        &mut self.inputs_scratch,
                    );
                    (&step.input_slots, &self.inputs_scratch)
                } else {
                    (&[], &[])
                };
            let ctx = build_eval_ctx(
                step,
                input_slots_view,
                input_values_view,
                &self.node_data,
                self.stroke_seed,
                self.dab_index,
            );
            f(&step.type_id, evaluator.as_ref(), &ctx, gpu);
        }
    }

    /// Read a named output slot value (for testing and downstream consumption).
    pub fn read_slot(&self, slot: usize) -> Option<ScalarValue> {
        self.slots.get(slot).copied().flatten()
    }

    /// Read the terminal's most recently published `dab_size` as a
    /// `(width, height)` pair of canvas pixels, or `None` if the graph
    /// has no terminal that publishes one yet. Each terminal owns the
    /// unit of dab_size it returns — paint/watercolor/smudge/liquify
    /// all return the disc diameter for stroke spacing. The stroke
    /// engine uses this to size both dab spacing and save-point bboxes.
    ///
    /// Resolved once at runner build (see `dab_size_slot`); per-dab
    /// cost is one slot read.
    pub fn last_dab_size(&self) -> Option<[f32; 2]> {
        let slot = self.dab_size_slot?;
        let size = self.read_slot(slot)?.as_vec2();
        (size[0] > 0.0 && size[1] > 0.0).then_some(size)
    }

    /// Find the slot index for a named output port on a specific step.
    ///
    /// Linear scan — intended for tests and debugging, not hot paths.
    pub fn find_output_slot(&self, type_id: &str, port_name: &str) -> Option<usize> {
        self.plan
            .steps
            .iter()
            .find(|s| s.type_id == type_id)
            .and_then(|s| {
                s.output_slots
                    .iter()
                    .find(|(name, _)| name == port_name)
                    .map(|(_, slot)| *slot)
            })
    }

    /// Find the slot index for a specific node's output port.
    ///
    /// Linear scan — intended for tests and debugging, not hot paths.
    pub fn find_node_output_slot(&self, node_id: NodeId, port_name: &str) -> Option<usize> {
        self.plan
            .steps
            .iter()
            .find(|s| s.node_id == node_id)
            .and_then(|s| {
                s.output_slots
                    .iter()
                    .find(|(name, _)| name == port_name)
                    .map(|(_, slot)| *slot)
            })
    }

    /// Access the execution plan.
    pub fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    /// Clear all slots for the next dab evaluation.
    pub fn clear_slots(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = None;
        }
    }

    /// True if any node in the graph has a `brush_preview` input that's
    /// connected (i.e. a wire targets some node's `brush_preview` port).
    /// `regenerate_brush_preview` uses this to short-circuit when no
    /// terminal asked to render a preview.
    pub fn graph_has_preview_wire(&self) -> bool {
        // Plan steps include input_slots for connected ports only
        // (disconnected ports fall back to defaults and never appear here),
        // so a `brush_preview` input slot is proof of a wire.
        self.plan.steps.iter().any(|s| {
            s.input_slots
                .iter()
                .any(|slot| slot.port_name == "brush_preview")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_identity_when_ranges_match() {
        for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!((remap_scalar(v, (0.0, 1.0), (0.0, 1.0)) - v).abs() < 1e-6);
        }
    }

    #[test]
    fn remap_unit_to_seed_range() {
        // 0..1 → 0..1024 — the canonical random → seed case.
        assert!((remap_scalar(0.0, (0.0, 1.0), (0.0, 1024.0)) - 0.0).abs() < 1e-4);
        assert!((remap_scalar(0.5, (0.0, 1.0), (0.0, 1024.0)) - 512.0).abs() < 1e-4);
        assert!((remap_scalar(1.0, (0.0, 1.0), (0.0, 1024.0)) - 1024.0).abs() < 1e-4);
    }

    #[test]
    fn remap_bipolar_to_seed_range() {
        // -1..1 → 0..1024 — old random output convention into seed.
        assert!((remap_scalar(-1.0, (-1.0, 1.0), (0.0, 1024.0)) - 0.0).abs() < 1e-4);
        assert!((remap_scalar(0.0, (-1.0, 1.0), (0.0, 1024.0)) - 512.0).abs() < 1e-4);
        assert!((remap_scalar(1.0, (-1.0, 1.0), (0.0, 1024.0)) - 1024.0).abs() < 1e-4);
    }

    #[test]
    fn remap_unit_to_bipolar_radians() {
        use std::f32::consts::TAU;
        // 0..1 → -TAU..TAU — random → phase.
        assert!((remap_scalar(0.0, (0.0, 1.0), (-TAU, TAU)) - (-TAU)).abs() < 1e-4);
        assert!((remap_scalar(0.5, (0.0, 1.0), (-TAU, TAU))).abs() < 1e-4);
        assert!((remap_scalar(1.0, (0.0, 1.0), (-TAU, TAU)) - TAU).abs() < 1e-4);
    }

    #[test]
    fn remap_degenerate_source_collapses_to_dst_min() {
        // Source range with zero width can't normalize — collapse cleanly
        // rather than producing NaN.
        assert_eq!(remap_scalar(0.5, (1.0, 1.0), (0.0, 1024.0)), 0.0);
        assert_eq!(remap_scalar(0.5, (1.0, 1.0), (-5.0, 5.0)), -5.0);
    }

    #[test]
    fn remap_outside_src_range_is_not_clamped() {
        // Consistent with the "ranges are UI hints" contract — overshoot
        // passes through. Consumers clamp inside their own evaluator if
        // they need a hard bound.
        let v = remap_scalar(1.5, (0.0, 1.0), (0.0, 100.0));
        assert!((v - 150.0).abs() < 1e-4);
    }
}
