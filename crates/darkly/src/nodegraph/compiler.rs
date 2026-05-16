use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::graph::{Graph, GraphError, NodeId, PortDir, PortRef};
use super::registration::NodeRegistration;
use super::WireKind;

/// A wired input on an execution step: which port, which slot to read from,
/// and which source port wrote to that slot. The `source` ref lets the
/// runner look up the source port's declared `natural_range` to remap
/// the value when the source and dest use different value ranges.
///
/// Only connected inputs appear in `ExecStep::input_slots` (disconnected
/// inputs fall back to their port default at eval time), so `source` is
/// always present — never `None`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputSlot {
    pub port_name: String,
    pub slot: usize,
    pub source: PortRef,
}

/// One step in a compiled execution plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecStep {
    /// Which node to evaluate.
    pub node_id: NodeId,
    /// The node's type_id (for looking up the evaluator).
    pub type_id: String,
    /// Whether this node runs on the GPU.
    pub is_gpu: bool,
    /// One entry per **connected** input port — disconnected inputs are
    /// resolved against the port's default at eval time and don't appear here.
    pub input_slots: Vec<InputSlot>,
    /// Mapping from each output port name → the slot index it writes to.
    pub output_slots: Vec<(String, usize)>,
}

/// A fully compiled, ready-to-execute plan for a graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub steps: Vec<ExecStep>,
    /// Total number of value slots required.
    pub slot_count: usize,
}

/// Compile a graph into a linear execution plan.
///
/// Uses Kahn's algorithm for topological sorting, then assigns flat
/// slot indices for inter-node data passing.  Returns `CycleDetected`
/// if the graph contains cycles (shouldn't happen if `Graph::connect`
/// is used, but handles corrupted/deserialized graphs).
pub fn compile<W: WireKind>(
    graph: &Graph<W>,
    registry: &HashMap<String, NodeRegistration<W>>,
) -> Result<ExecutionPlan, GraphError> {
    let node_ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
    if node_ids.is_empty() {
        return Ok(ExecutionPlan {
            steps: vec![],
            slot_count: 0,
        });
    }

    // ── Kahn's topological sort ──────────────────────────────────────

    // Build in-degree map from connections.
    let mut in_degree: HashMap<NodeId, usize> = node_ids.iter().map(|&id| (id, 0)).collect();
    for conn in &graph.connections {
        *in_degree.entry(conn.to.node).or_default() += 1;
    }

    let mut queue: Vec<NodeId> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    // Sort for deterministic ordering.
    queue.sort_by_key(|id| id.0);

    let mut sorted = Vec::with_capacity(node_ids.len());

    while let Some(id) = queue.pop() {
        sorted.push(id);
        // Decrement in-degree for each connection from this node.
        // Do NOT dedup: in-degree was counted per-connection, so we must
        // decrement once per connection too.
        let mut downstream: Vec<NodeId> = graph
            .connections
            .iter()
            .filter(|c| c.from.node == id)
            .map(|c| c.to.node)
            .collect();
        downstream.sort_by_key(|id| id.0);

        for &next in &downstream {
            let deg = in_degree.get_mut(&next).unwrap();
            *deg -= 1;
            if *deg == 0 {
                // Insert sorted to keep queue deterministic.
                let pos = queue.partition_point(|q| q.0 > next.0);
                queue.insert(pos, next);
            }
        }
    }

    if sorted.len() != node_ids.len() {
        return Err(GraphError::CycleDetected);
    }

    // ── Slot allocation ──────────────────────────────────────────────

    // Each output port gets a unique slot.  Input ports are mapped to
    // the slot of whatever output port feeds them (via connections).
    let mut next_slot: usize = 0;
    let mut output_slot_map: HashMap<PortRef, usize> = HashMap::new();

    // First pass: assign slots to all output ports.
    for &node_id in &sorted {
        let node = &graph.nodes[&node_id];
        for port in &node.ports {
            if port.dir == PortDir::Output {
                let pr = PortRef {
                    node: node_id,
                    port: port.name.clone(),
                };
                output_slot_map.insert(pr, next_slot);
                next_slot += 1;
            }
        }
    }

    // Build input→output lookup from connections.
    let mut input_wire: HashMap<PortRef, PortRef> = HashMap::new();
    for conn in &graph.connections {
        input_wire.insert(conn.to.clone(), conn.from.clone());
    }

    // ── Build steps ──────────────────────────────────────────────────
    //
    // `is_gpu` phases execution: GPU-phase steps run after the CPU-phase
    // batch and have access to the GPU context. A step is in the GPU
    // phase if *either* its registration declares `is_gpu: true` (e.g.
    // `stamp` because it records render passes) *or* any of its inputs
    // is produced by a GPU-phase step (so we can't evaluate until those
    // outputs exist). The second case auto-promotes pure-math helpers
    // like `split_vec2` whenever they sit downstream of a GPU node —
    // the author doesn't have to know, the graph topology decides.

    let mut steps = Vec::with_capacity(sorted.len());
    let mut phase: HashMap<NodeId, bool> = HashMap::new();

    for &node_id in &sorted {
        let node = &graph.nodes[&node_id];
        let declared_gpu = registry
            .get(&node.type_id)
            .map(|r| r.is_gpu)
            .unwrap_or(false);

        // Promote to GPU phase if any upstream producer is already GPU.
        let inherits_gpu = node.ports.iter().any(|p| {
            if p.dir != PortDir::Input {
                return false;
            }
            let pr = PortRef {
                node: node_id,
                port: p.name.clone(),
            };
            input_wire
                .get(&pr)
                .and_then(|src| phase.get(&src.node))
                .copied()
                .unwrap_or(false)
        });
        let is_gpu = declared_gpu || inherits_gpu;
        phase.insert(node_id, is_gpu);

        let mut input_slots = Vec::new();
        let mut output_slots = Vec::new();

        for port in &node.ports {
            match port.dir {
                PortDir::Input => {
                    let pr = PortRef {
                        node: node_id,
                        port: port.name.clone(),
                    };
                    if let Some(src) = input_wire.get(&pr) {
                        let slot = output_slot_map[src];
                        input_slots.push(InputSlot {
                            port_name: port.name.clone(),
                            slot,
                            source: src.clone(),
                        });
                    }
                    // Disconnected inputs use their default value — the
                    // evaluator handles that (no slot assigned).
                }
                PortDir::Output => {
                    let pr = PortRef {
                        node: node_id,
                        port: port.name.clone(),
                    };
                    let slot = output_slot_map[&pr];
                    output_slots.push((port.name.clone(), slot));
                }
            }
        }

        steps.push(ExecStep {
            node_id,
            type_id: node.type_id.clone(),
            is_gpu,
            input_slots,
            output_slots,
        });
    }

    Ok(ExecutionPlan {
        steps,
        slot_count: next_slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodegraph::graph::{Graph, PortDef, PortRef};
    use crate::nodegraph::registration::NodeRegistration;
    use crate::nodegraph::tests::TestWireKind;

    fn test_registry() -> HashMap<String, NodeRegistration<TestWireKind>> {
        let mut map = HashMap::new();
        map.insert(
            "source".into(),
            NodeRegistration {
                type_id: "source",
                category: "test",
                display_name: "Source",
                ports: vec![PortDef::output("out", TestWireKind::Scalar)],
                params: &[],
                is_gpu: false,
            },
        );
        map.insert(
            "passthrough".into(),
            NodeRegistration {
                type_id: "passthrough",
                category: "test",
                display_name: "Passthrough",
                ports: vec![
                    PortDef::input("in", TestWireKind::Scalar),
                    PortDef::output("out", TestWireKind::Scalar),
                ],
                params: &[],
                is_gpu: false,
            },
        );
        map.insert(
            "sink".into(),
            NodeRegistration {
                type_id: "sink",
                category: "test",
                display_name: "Sink",
                ports: vec![PortDef::input("in", TestWireKind::Scalar)],
                params: &[],
                is_gpu: false,
            },
        );
        map
    }

    #[test]
    fn topological_sort_linear_chain() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node(
            "source",
            vec![PortDef::output("out", TestWireKind::Scalar)],
            vec![],
        );
        let b = g.add_node(
            "passthrough",
            vec![
                PortDef::input("in", TestWireKind::Scalar),
                PortDef::output("out", TestWireKind::Scalar),
            ],
            vec![],
        );
        let c = g.add_node(
            "sink",
            vec![PortDef::input("in", TestWireKind::Scalar)],
            vec![],
        );

        g.connect(
            PortRef {
                node: a,
                port: "out".into(),
            },
            PortRef {
                node: b,
                port: "in".into(),
            },
        )
        .unwrap();
        g.connect(
            PortRef {
                node: b,
                port: "out".into(),
            },
            PortRef {
                node: c,
                port: "in".into(),
            },
        )
        .unwrap();

        let plan = compile(&g, &test_registry()).unwrap();

        assert_eq!(plan.steps.len(), 3);

        // a must come before b, b before c.
        let pos = |id: NodeId| plan.steps.iter().position(|s| s.node_id == id).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(b) < pos(c));
    }

    #[test]
    fn slot_indices_wired_correctly() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node(
            "source",
            vec![PortDef::output("out", TestWireKind::Scalar)],
            vec![],
        );
        let b = g.add_node(
            "sink",
            vec![PortDef::input("in", TestWireKind::Scalar)],
            vec![],
        );

        g.connect(
            PortRef {
                node: a,
                port: "out".into(),
            },
            PortRef {
                node: b,
                port: "in".into(),
            },
        )
        .unwrap();

        let plan = compile(&g, &test_registry()).unwrap();

        let a_step = plan.steps.iter().find(|s| s.node_id == a).unwrap();
        let b_step = plan.steps.iter().find(|s| s.node_id == b).unwrap();

        // a's output slot should match b's input slot.
        let a_out_slot = a_step.output_slots[0].1;
        let b_in_slot = b_step.input_slots[0].slot;
        assert_eq!(a_out_slot, b_in_slot);
        // ...and b's input must record `a.out` as its source port.
        assert_eq!(b_step.input_slots[0].source.node, a);
        assert_eq!(b_step.input_slots[0].source.port, "out");
    }

    #[test]
    fn diamond_graph() {
        // A → B, A → C, B → D, C → D
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node(
            "source",
            vec![PortDef::output("out", TestWireKind::Scalar)],
            vec![],
        );
        let b = g.add_node(
            "passthrough",
            vec![
                PortDef::input("in", TestWireKind::Scalar),
                PortDef::output("out", TestWireKind::Scalar),
            ],
            vec![],
        );
        let c = g.add_node(
            "passthrough",
            vec![
                PortDef::input("in", TestWireKind::Scalar),
                PortDef::output("out", TestWireKind::Scalar),
            ],
            vec![],
        );
        let d = g.add_node(
            "sink",
            vec![
                PortDef::input("in_a", TestWireKind::Scalar),
                PortDef::input("in_b", TestWireKind::Scalar),
            ],
            vec![],
        );

        g.connect(
            PortRef {
                node: a,
                port: "out".into(),
            },
            PortRef {
                node: b,
                port: "in".into(),
            },
        )
        .unwrap();
        g.connect(
            PortRef {
                node: a,
                port: "out".into(),
            },
            PortRef {
                node: c,
                port: "in".into(),
            },
        )
        .unwrap();
        g.connect(
            PortRef {
                node: b,
                port: "out".into(),
            },
            PortRef {
                node: d,
                port: "in_a".into(),
            },
        )
        .unwrap();
        g.connect(
            PortRef {
                node: c,
                port: "out".into(),
            },
            PortRef {
                node: d,
                port: "in_b".into(),
            },
        )
        .unwrap();

        let plan = compile(&g, &test_registry()).unwrap();
        assert_eq!(plan.steps.len(), 4);

        let pos = |id: NodeId| plan.steps.iter().position(|s| s.node_id == id).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(a) < pos(c));
        assert!(pos(b) < pos(d));
        assert!(pos(c) < pos(d));
    }

    #[test]
    fn empty_graph() {
        let g = Graph::<TestWireKind>::new();
        let plan = compile(&g, &test_registry()).unwrap();
        assert_eq!(plan.steps.len(), 0);
        assert_eq!(plan.slot_count, 0);
    }

    #[test]
    fn multi_edge_fanout_to_same_node() {
        // A has two outputs, both feeding into B's two inputs.
        // This must not be detected as a cycle.
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node(
            "source",
            vec![
                PortDef::output("out1", TestWireKind::Scalar),
                PortDef::output("out2", TestWireKind::Scalar),
            ],
            vec![],
        );
        let b = g.add_node(
            "sink",
            vec![
                PortDef::input("in1", TestWireKind::Scalar),
                PortDef::input("in2", TestWireKind::Scalar),
            ],
            vec![],
        );

        g.connect(
            PortRef {
                node: a,
                port: "out1".into(),
            },
            PortRef {
                node: b,
                port: "in1".into(),
            },
        )
        .unwrap();
        g.connect(
            PortRef {
                node: a,
                port: "out2".into(),
            },
            PortRef {
                node: b,
                port: "in2".into(),
            },
        )
        .unwrap();

        let mut reg = test_registry();
        reg.insert(
            "source".into(),
            NodeRegistration {
                type_id: "source",
                category: "test",
                display_name: "Source",
                ports: vec![
                    PortDef::output("out1", TestWireKind::Scalar),
                    PortDef::output("out2", TestWireKind::Scalar),
                ],
                params: &[],
                is_gpu: false,
            },
        );
        reg.insert(
            "sink".into(),
            NodeRegistration {
                type_id: "sink",
                category: "test",
                display_name: "Sink",
                ports: vec![
                    PortDef::input("in1", TestWireKind::Scalar),
                    PortDef::input("in2", TestWireKind::Scalar),
                ],
                params: &[],
                is_gpu: false,
            },
        );

        let plan = compile(&g, &reg).unwrap();
        assert_eq!(plan.steps.len(), 2);

        let pos = |id: NodeId| plan.steps.iter().position(|s| s.node_id == id).unwrap();
        assert!(pos(a) < pos(b));
    }
}
