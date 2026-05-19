//! Brush graph evaluation runtime.
//!
//! The runner takes a compiled `ExecutionPlan`, a table of evaluator
//! closures, and a flat `Vec<Option<ScalarValue>>` slot table.  Per-dab
//! evaluation is zero-heap-allocation: the slot table is pre-sized and
//! reused across dabs.

use std::collections::HashMap;

use crate::gpu::params::ParamValue;
use crate::nodegraph::{
    ExecutionPlan, Graph, InputSlot, NodeId, NodeRegistration, PortDef, PortDir, PortRef,
};

use super::curve_math::CurveLut;

use super::gpu_context::BrushGpuContext;
use super::wire::{BrushWireType, ScalarValue};

// ── Evaluator trait ─────────────────────────────────────────────────

/// Context passed to each node's CPU evaluator.
///
/// Inputs are gathered into a HashMap by the runner before calling the
/// evaluator, rather than giving evaluators direct slot table access.
/// This keeps evaluators decoupled from the slot layout — they just ask
/// for named inputs and get values back, without knowing slot indices.
/// The HashMap allocation is per-node-per-dab, but node input counts
/// are tiny (1-3 ports), so this is negligible.
pub struct EvalContext<'a> {
    /// Read a named input port.  Returns `None` for disconnected ports.
    pub inputs: &'a HashMap<String, ScalarValue>,
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
        if let Some(&val) = self.inputs.get(name) {
            return val;
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

/// Gather a step's connected inputs from the slot table, applying
/// wire-boundary range remap. This is where the "everything speaks 0-1"
/// intent in [`crate::brush::wire`] actually lives: when both ends of a
/// wire declare a `natural_range`, the value gets affinely remapped at
/// the boundary; otherwise it passes through raw (preserving math-node
/// and over-drag-slider passthrough).
fn gather_inputs(
    slots: &[Option<ScalarValue>],
    input_slots: &[InputSlot],
    dest_node: NodeId,
    node_data: &HashMap<NodeId, NodeData>,
) -> HashMap<String, ScalarValue> {
    let mut inputs = HashMap::with_capacity(input_slots.len());
    for slot_info in input_slots {
        let Some(val) = slots[slot_info.slot] else {
            continue;
        };
        let remapped = remap_for_wire(
            val,
            &slot_info.source,
            dest_node,
            &slot_info.port_name,
            node_data,
        );
        inputs.insert(slot_info.port_name.clone(), remapped);
    }
    inputs
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
    /// Like `begin_stroke`, inputs here are port defaults only — the
    /// committed state is a function of the scratch, which already
    /// reflects all per-dab CPU input resolution.
    fn commit(&self, _ctx: &EvalContext, _gpu: &mut BrushGpuContext) {}

    /// Does this terminal honor `BrushGpuContext::blend_mode` (paint vs.
    /// erase)? Default `true` — most terminals do, and non-terminal
    /// nodes never see blend_mode so the value is unread for them.
    /// Output terminals that ignore the flag (liquify, watercolor,
    /// smudge — erase semantics don't apply to warp/smear) override to
    /// `false` so the brush-tool UI can hide the erase toggle.
    /// `active_brush_supports_erase` ANDs across each output node's
    /// flag to decide whether the active brush supports erase at all.
    fn supports_erase(&self) -> bool {
        true
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
    /// Evaluator for each node type_id.  Looked up once per step during
    /// `execute_cpu()` — the HashMap cost is acceptable because the number
    /// of steps per dab is small (typically 5-15 nodes).
    evaluators: HashMap<String, Box<dyn BrushNodeEvaluator>>,
    /// Flat slot table indexed by compiler-assigned slot number.  Pre-sized
    /// to `plan.slot_count` and reused across dabs — `clear_slots()` resets
    /// it between evaluations without reallocating.
    slots: Vec<Option<ScalarValue>>,
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
    /// PRNG seed for the current stroke, set by `seed_sensors()`.
    stroke_seed: u32,
    /// Index of the current dab, set by `seed_sensors()`.
    dab_index: u32,
}

struct NodeData {
    params: Vec<ParamValue>,
    port_defs: Vec<PortDef<BrushWireType>>,
    lut: Option<CurveLut>,
}

impl BrushGraphRunner {
    /// Build a runner from a graph and a registry of evaluators.
    pub fn new(
        graph: &Graph<BrushWireType>,
        registry: &HashMap<String, NodeRegistration<BrushWireType>>,
        evaluators: HashMap<String, Box<dyn BrushNodeEvaluator>>,
    ) -> Result<Self, crate::nodegraph::GraphError> {
        let plan = crate::nodegraph::compile(graph, registry)?;
        let slots = vec![None; plan.slot_count];

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
            .find(|s| s.type_id == "pen_input")
            .map(|s| s.output_slots.clone())
            .unwrap_or_default();

        // Find paint_color node's color output slot.
        let paint_color_slot = plan
            .steps
            .iter()
            .find(|s| s.type_id == "paint_color")
            .and_then(|s| s.output_slots.iter().find(|(name, _)| name == "color"))
            .map(|(_, slot)| *slot);

        Ok(Self {
            plan,
            evaluators,
            slots,
            node_data,
            pen_input_slots,
            paint_color_slot,
            stroke_seed: 0,
            dab_index: 0,
        })
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
        for step in &self.plan.steps {
            // Skip pen_input (seeded directly) and GPU nodes.
            if step.type_id == "pen_input" || step.type_id == "paint_color" || step.is_gpu {
                continue;
            }

            let Some(evaluator) = self.evaluators.get(&step.type_id) else {
                continue;
            };

            // Gather connected inputs from the slot table, applying
            // wire-boundary range remap where both source and dest ports
            // declare a `natural_range`.
            let inputs = gather_inputs(
                &self.slots,
                &step.input_slots,
                step.node_id,
                &self.node_data,
            );

            let node = self.node_data.get(&step.node_id);
            let empty_params = Vec::new();
            let empty_ports = Vec::new();
            let ctx = EvalContext {
                inputs: &inputs,
                params: node.map(|n| n.params.as_slice()).unwrap_or(&empty_params),
                port_defs: node.map(|n| n.port_defs.as_slice()).unwrap_or(&empty_ports),
                lut: node.and_then(|n| n.lut.as_ref()),
                stroke_seed: self.stroke_seed,
                dab_index: self.dab_index,
                node_id: step.node_id,
            };

            let outputs = evaluator.evaluate_cpu(&ctx);

            // Write outputs to their assigned slots.
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
    ///
    /// After this returns, call `gpu.dab_pool.release_all()` to return
    /// acquired dab textures to the pool.
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
        for step in &self.plan.steps {
            if !step.is_gpu {
                continue;
            }

            let Some(evaluator) = self.evaluators.get(&step.type_id) else {
                continue;
            };

            gpu.perf.record_gpu_step();

            // Gather connected inputs from the slot table, applying
            // wire-boundary range remap where both source and dest ports
            // declare a `natural_range`. Allocates a fresh `HashMap` per
            // step; under high dab counts the cumulative allocator +
            // remap-lookup cost shows up in the perf summary.
            let t_gather = web_time::Instant::now();
            let inputs = gather_inputs(
                &self.slots,
                &step.input_slots,
                step.node_id,
                &self.node_data,
            );
            gpu.perf
                .record_gather_inputs(t_gather.elapsed().as_micros() as u64);

            let node = self.node_data.get(&step.node_id);
            let empty_params = Vec::new();
            let empty_ports = Vec::new();
            let ctx = EvalContext {
                inputs: &inputs,
                params: node.map(|n| n.params.as_slice()).unwrap_or(&empty_params),
                port_defs: node.map(|n| n.port_defs.as_slice()).unwrap_or(&empty_ports),
                lut: node.and_then(|n| n.lut.as_ref()),
                stroke_seed: self.stroke_seed,
                dab_index: self.dab_index,
                node_id: step.node_id,
            };

            // Pure-math nodes promoted to the GPU phase (because an input
            // depends on a GPU output) only implement `evaluate_cpu`. Run
            // it here so the slot table fills in topological order; the
            // `evaluate_gpu` closure runs too and no-ops (empty default).
            // Declared-GPU nodes take the opposite path: `evaluate_cpu`
            // returns empty, `evaluate_gpu` does the work.
            let t_cpu = web_time::Instant::now();
            let mut outputs = evaluator.evaluate_cpu(&ctx);
            let cpu_us = t_cpu.elapsed().as_micros() as u64;
            gpu.perf.record_evaluate_cpu_in_gpu(cpu_us);

            let t_gpu_call = web_time::Instant::now();
            let gpu_outputs = f(evaluator.as_ref(), &ctx, gpu);
            let gpu_call_us = t_gpu_call.elapsed().as_micros() as u64;
            gpu.perf.record_evaluate_gpu_call(gpu_call_us);

            outputs.extend(gpu_outputs);

            // Write outputs to their assigned slots. Linear scan with
            // string compare per produced output × per step × per dab —
            // a candidate hot spot at high dab counts.
            let t_outputs = web_time::Instant::now();
            for (port_name, value) in outputs {
                for (name, slot_idx) in &step.output_slots {
                    if *name == port_name {
                        self.slots[*slot_idx] = Some(value);
                        break;
                    }
                }
            }
            gpu.perf
                .record_step_outputs(t_outputs.elapsed().as_micros() as u64);
        }
    }

    /// Dispatch `begin_stroke` to every GPU node's evaluator in topological
    /// order. Only terminal nodes override — everything else no-ops. Runs
    /// once per stroke-start and once per rewind boundary, before any dab.
    pub fn begin_stroke(&mut self, gpu: &mut BrushGpuContext) {
        self.dispatch_lifecycle(gpu, |ev, ctx, gpu| ev.begin_stroke(ctx, gpu));
    }

    /// Dispatch `commit` to every GPU node's evaluator in topological
    /// order. Runs once per pen event after that event's dabs have
    /// finished compositing into the scratch.
    pub fn commit(&mut self, gpu: &mut BrushGpuContext) {
        self.dispatch_lifecycle(gpu, |ev, ctx, gpu| ev.commit(ctx, gpu));
    }

    /// Shared walker for lifecycle hooks. Mirrors `execute_gpu` minus the
    /// per-dab slot/input plumbing, because lifecycle hooks run *outside*
    /// any specific dab — they work from port defaults and GPU resources.
    fn dispatch_lifecycle<F>(&mut self, gpu: &mut BrushGpuContext, mut f: F)
    where
        F: FnMut(&dyn BrushNodeEvaluator, &EvalContext, &mut BrushGpuContext),
    {
        let empty_inputs = HashMap::new();
        let empty_params = Vec::new();
        let empty_ports = Vec::new();
        for step in &self.plan.steps {
            if !step.is_gpu {
                continue;
            }
            let Some(evaluator) = self.evaluators.get(&step.type_id) else {
                continue;
            };
            let node = self.node_data.get(&step.node_id);
            let ctx = EvalContext {
                inputs: &empty_inputs,
                params: node.map(|n| n.params.as_slice()).unwrap_or(&empty_params),
                port_defs: node.map(|n| n.port_defs.as_slice()).unwrap_or(&empty_ports),
                lut: node.and_then(|n| n.lut.as_ref()),
                stroke_seed: self.stroke_seed,
                dab_index: self.dab_index,
                node_id: step.node_id,
            };
            f(evaluator.as_ref(), &ctx, gpu);
        }
    }

    /// Read a named output slot value (for testing and downstream consumption).
    pub fn read_slot(&self, slot: usize) -> Option<ScalarValue> {
        self.slots.get(slot).copied().flatten()
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
