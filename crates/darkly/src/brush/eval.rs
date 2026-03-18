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
pub struct BrushGraphRunner {
    /// The compiled execution plan.
    plan: ExecutionPlan,
    /// Evaluator for each node type_id.
    evaluators: HashMap<String, Box<dyn BrushNodeEvaluator>>,
    /// Flat slot table — pre-sized, reused across dabs.
    slots: Vec<Option<ScalarValue>>,
    /// Per-node instance data: (type_id, params, port_defs).
    node_data: HashMap<NodeId, NodeData>,
    /// Slot indices for pen_input sensor outputs (for direct seeding).
    pen_input_slots: Vec<(String, usize)>,
    /// Slot index for the paint_color node's color output.
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
