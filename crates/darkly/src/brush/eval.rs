//! Brush graph evaluation runtime.
//!
//! The runner takes a compiled `ExecutionPlan`, a table of evaluator
//! closures, and a flat `Vec<Option<ScalarValue>>` slot table.  Per-dab
//! evaluation is zero-heap-allocation: the slot table is pre-sized and
//! reused across dabs.

use std::collections::HashMap;

use crate::gpu::params::ParamValue;
use crate::nodegraph::{ExecutionPlan, Graph, NodeId, NodeRegistration, PortDef, PortDir};

use super::paint_info::PaintInformation;
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
}

/// Trait implemented by each CPU node to produce output values.
pub trait BrushNodeEvaluator: Send + Sync {
    /// Evaluate the node and return named output values.
    ///
    /// Called once per dab for each CPU node in topological order.
    /// GPU nodes skip this — they're handled by `execute_gpu` (Phase 3).
    fn evaluate_cpu(
        &self,
        ctx: &EvalContext,
    ) -> Vec<(String, ScalarValue)>;
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
///    node's evaluator which reads inputs from and writes outputs to the
///    flat slot table.
/// 3. Downstream consumers (GPU stage nodes in Phase 3) read final values
///    from the slot table by index.
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
}

struct NodeData {
    params: Vec<ParamValue>,
    port_defs: Vec<PortDef<BrushWireType>>,
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
        let mut node_data = HashMap::new();
        for step in &plan.steps {
            if let Some(node) = graph.nodes.get(&step.node_id) {
                node_data.insert(step.node_id, NodeData {
                    params: node.params.clone(),
                    port_defs: node.ports.clone(),
                });
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
        })
    }

    /// Seed sensor output slots directly from pen data.
    ///
    /// This is the hot path — no virtual dispatch, just memcpy into
    /// pre-known slot indices.
    pub fn seed_sensors(&mut self, info: &PaintInformation, color: [f32; 4]) {
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
                "index" => ScalarValue::Int(info.index as i32),
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

            // Gather connected inputs from the slot table.
            let mut inputs = HashMap::new();
            for (port_name, slot_idx) in &step.input_slots {
                if let Some(val) = self.slots[*slot_idx] {
                    inputs.insert(port_name.clone(), val);
                }
            }

            let node = self.node_data.get(&step.node_id);
            let empty_params = Vec::new();
            let empty_ports = Vec::new();
            let ctx = EvalContext {
                inputs: &inputs,
                params: node.map(|n| n.params.as_slice()).unwrap_or(&empty_params),
                port_defs: node.map(|n| n.port_defs.as_slice()).unwrap_or(&empty_ports),
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
}
