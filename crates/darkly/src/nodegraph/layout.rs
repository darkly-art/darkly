//! Automatic layout for node graphs.
//!
//! Implements a Sugiyama-style layered layout: longest-path layer
//! assignment, barycenter crossing minimization, and left-to-right
//! position assignment.  Works on any `Graph<W>` without requiring
//! a domain-specific registry.

use std::collections::{HashMap, HashSet, VecDeque};

use super::graph::{Graph, NodeId, PortDir};
use super::WireKind;

/// Horizontal spacing between layers (pixels).
const SPACING_X: f32 = 220.0;

/// Vertical gap between nodes within a layer (pixels).
const GAP_Y: f32 = 30.0;

/// Minimum node height when port count is unknown.
const MIN_NODE_H: f32 = 50.0;

// Matches canvas_renderer.ts constants for accurate height estimation.
const NODE_HEADER_H: f32 = 24.0;
const PORT_ROW_H: f32 = 18.0;
const BODY_PAD: f32 = 4.0;

/// Number of barycenter sweeps for crossing minimization.
const SWEEPS: usize = 4;

/// Build a position index: node → position within its layer.
fn build_index(layers: &[Vec<NodeId>]) -> HashMap<NodeId, usize> {
    let mut idx = HashMap::new();
    for layer in layers {
        for (pos, &id) in layer.iter().enumerate() {
            idx.insert(id, pos);
        }
    }
    idx
}

/// Reorder `layers[layer_idx]` by barycenter positions from `neighbor_layer`.
/// `adj` maps each node to its neighbors in the reference layer.
fn reorder_by_barycenter(
    layers: &mut [Vec<NodeId>],
    layer_idx: usize,
    adj: &HashMap<NodeId, Vec<NodeId>>,
    index: &HashMap<NodeId, usize>,
) {
    let mut barycenters: Vec<(NodeId, f64)> = Vec::new();
    for &node in &layers[layer_idx] {
        let bc = if let Some(neighbors) = adj.get(&node) {
            let positions: Vec<f64> = neighbors
                .iter()
                .filter_map(|&n| index.get(&n).map(|&i| i as f64))
                .collect();
            if positions.is_empty() {
                index.get(&node).copied().unwrap_or(0) as f64
            } else {
                positions.iter().sum::<f64>() / positions.len() as f64
            }
        } else {
            index.get(&node).copied().unwrap_or(0) as f64
        };
        barycenters.push((node, bc));
    }
    barycenters.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    layers[layer_idx] = barycenters.into_iter().map(|(id, _)| id).collect();
}

/// Assign y positions to a vertical column of nodes, centered around y=0.
fn assign_column(
    nodes: &mut HashMap<NodeId, super::graph::NodeInstance<impl WireKind>>,
    ids: &[NodeId],
    x: f32,
    heights: &HashMap<NodeId, f32>,
) {
    let h: Vec<f32> = ids
        .iter()
        .map(|id| heights.get(id).copied().unwrap_or(MIN_NODE_H))
        .collect();
    let total_h: f32 = h.iter().sum::<f32>() + ids.len().saturating_sub(1) as f32 * GAP_Y;
    let mut y = -total_h / 2.0;
    for (i, &id) in ids.iter().enumerate() {
        if let Some(node) = nodes.get_mut(&id) {
            node.position = [x, y];
        }
        y += h[i] + GAP_Y;
    }
}

impl<W: WireKind> Graph<W> {
    /// Compute and assign positions for all nodes using a layered layout.
    ///
    /// Data flows left-to-right: source nodes at x=0, downstream nodes
    /// at increasing x.  Nodes within a layer are spaced vertically and
    /// ordered to minimize edge crossings.
    ///
    /// When no measured sizes are available (tests, freshly loaded brushes),
    /// falls back to port-count-based height estimation.
    pub fn auto_layout(&mut self) {
        self.auto_layout_with_sizes(&HashMap::new());
    }

    /// Like [`auto_layout`], but uses measured `[width, height]` from
    /// the DOM for any node present in `sizes`.  Nodes not in the map
    /// fall back to port-count estimation.
    pub fn auto_layout_with_sizes(&mut self, sizes: &HashMap<NodeId, [f32; 2]>) {
        if self.nodes.is_empty() {
            return;
        }

        // ── Build adjacency ─────────────────────────────────────────
        let mut forward: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut reverse: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut connected: HashSet<NodeId> = HashSet::new();

        for conn in &self.connections {
            forward
                .entry(conn.from.node)
                .or_default()
                .push(conn.to.node);
            reverse
                .entry(conn.to.node)
                .or_default()
                .push(conn.from.node);
            connected.insert(conn.from.node);
            connected.insert(conn.to.node);
        }

        // Deduplicate adjacency lists (multiple ports between same pair).
        for list in forward.values_mut() {
            list.sort_by_key(|id| id.0);
            list.dedup();
        }
        for list in reverse.values_mut() {
            list.sort_by_key(|id| id.0);
            list.dedup();
        }

        // ── Layer assignment (longest path from sources) ────────────
        // Inline Kahn's topo sort with longest-path DP.
        //
        // In-degree is computed at the *node* level (from the deduplicated
        // adjacency), not at the connection/port level.  Multiple wires
        // between the same pair of nodes count as one edge for topo sort.

        let mut in_degree: HashMap<NodeId, usize> = HashMap::new();
        for &id in &connected {
            in_degree.entry(id).or_insert(0);
        }
        // Each entry in `reverse` is already deduplicated, so its length
        // is the node-level in-degree.
        for (&node, preds) in &reverse {
            in_degree.insert(node, preds.len());
        }

        let mut sources: Vec<NodeId> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();
        sources.sort_by_key(|id| id.0);

        let mut layer: HashMap<NodeId, usize> = HashMap::new();
        let mut queue: VecDeque<NodeId> = VecDeque::new();
        for &src in &sources {
            layer.insert(src, 0);
            queue.push_back(src);
        }

        while let Some(id) = queue.pop_front() {
            let current_layer = layer[&id];
            if let Some(successors) = forward.get(&id) {
                for &next in successors {
                    let entry = layer.entry(next).or_insert(0);
                    *entry = (*entry).max(current_layer + 1);

                    let deg = in_degree.get_mut(&next).unwrap();
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push_back(next);
                    }
                }
            }
        }

        // ── Group nodes by layer ────────────────────────────────────

        let max_layer = layer.values().copied().max().unwrap_or(0);
        let mut layers: Vec<Vec<NodeId>> = vec![vec![]; max_layer + 1];
        for (&id, &l) in &layer {
            layers[l].push(id);
        }
        // Deterministic initial order.
        for layer_nodes in &mut layers {
            layer_nodes.sort_by_key(|id| id.0);
        }

        // ── Barycenter crossing minimization ────────────────────────

        for sweep in 0..SWEEPS {
            let index = build_index(&layers);
            if sweep % 2 == 0 {
                for l in 1..layers.len() {
                    reorder_by_barycenter(&mut layers, l, &reverse, &index);
                }
            } else {
                for l in (0..layers.len().saturating_sub(1)).rev() {
                    reorder_by_barycenter(&mut layers, l, &forward, &index);
                }
            }
        }

        // ── Compute node sizes ─────────────────────────────────────
        // Use DOM-measured sizes when available, else estimate from ports.

        let mut widths: HashMap<NodeId, f32> = HashMap::new();
        let mut heights: HashMap<NodeId, f32> = HashMap::new();
        for (&id, node) in &self.nodes {
            if let Some(&[w, h]) = sizes.get(&id) {
                widths.insert(id, w);
                heights.insert(id, h);
            } else {
                let n_in = node
                    .ports
                    .iter()
                    .filter(|p| p.dir == PortDir::Input)
                    .count();
                let n_out = node
                    .ports
                    .iter()
                    .filter(|p| p.dir == PortDir::Output)
                    .count();
                let max_ports = n_in.max(n_out);
                let h = NODE_HEADER_H + BODY_PAD * 2.0 + max_ports as f32 * PORT_ROW_H;
                heights.insert(id, h.max(MIN_NODE_H));
            }
        }

        // ── Assign positions ────────────────────────────────────────
        // Per-layer x uses the widest node in the preceding layer + gap,
        // falling back to SPACING_X when no widths are measured.

        let mut x = 0.0f32;
        for (l, layer_nodes) in layers.iter().enumerate() {
            if l > 0 {
                // Width of widest node in the previous layer.
                let prev_max_w = layers[l - 1]
                    .iter()
                    .filter_map(|id| widths.get(id))
                    .copied()
                    .fold(0.0f32, f32::max);
                x += if prev_max_w > 0.0 {
                    prev_max_w + GAP_Y * 2.0
                } else {
                    SPACING_X
                };
            }
            assign_column(&mut self.nodes, layer_nodes, x, &heights);
        }

        // ── Disconnected nodes ──────────────────────────────────────

        let mut disconnected: Vec<NodeId> = self
            .nodes
            .keys()
            .filter(|id| !connected.contains(id))
            .copied()
            .collect();
        disconnected.sort_by_key(|id| id.0);

        if !disconnected.is_empty() {
            let disc_x = if connected.is_empty() {
                0.0
            } else {
                // Place after the last connected layer.
                x + SPACING_X
            };
            assign_column(&mut self.nodes, &disconnected, disc_x, &heights);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodegraph::graph::{PortDef, PortRef};
    use crate::nodegraph::tests::TestWireKind;

    fn scalar_in(name: &str) -> PortDef<TestWireKind> {
        PortDef::input(name, TestWireKind::Scalar)
    }
    fn scalar_out(name: &str) -> PortDef<TestWireKind> {
        PortDef::output(name, TestWireKind::Scalar)
    }
    fn wire(g: &mut Graph<TestWireKind>, from: NodeId, from_port: &str, to: NodeId, to_port: &str) {
        g.connect(
            PortRef {
                node: from,
                port: from_port.into(),
            },
            PortRef {
                node: to,
                port: to_port.into(),
            },
        )
        .unwrap();
    }
    fn pos(g: &Graph<TestWireKind>, id: NodeId) -> [f32; 2] {
        g.nodes[&id].position
    }

    #[test]
    fn empty_graph() {
        let mut g = Graph::<TestWireKind>::new();
        g.auto_layout(); // should not panic
        assert!(g.nodes.is_empty());
    }

    #[test]
    fn single_node() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        g.auto_layout();
        // Disconnected single node at x=0, centered vertically.
        assert_eq!(pos(&g, a)[0], 0.0);
    }

    #[test]
    fn linear_chain() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let c = g.add_node("c", vec![scalar_in("in")], vec![]);
        wire(&mut g, a, "out", b, "in");
        wire(&mut g, b, "out", c, "in");

        g.auto_layout();

        // Positions increase in x; single-node layers are vertically centered.
        assert!(pos(&g, a)[0] < pos(&g, b)[0]);
        assert!(pos(&g, b)[0] < pos(&g, c)[0]);
    }

    #[test]
    fn diamond() {
        // A → B, A → C, B → D, C → D
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let c = g.add_node("c", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let d = g.add_node("d", vec![scalar_in("in_a"), scalar_in("in_b")], vec![]);
        wire(&mut g, a, "out", b, "in");
        wire(&mut g, a, "out", c, "in");
        wire(&mut g, b, "out", d, "in_a");
        wire(&mut g, c, "out", d, "in_b");

        g.auto_layout();

        // A at layer 0, B and C at layer 1, D at layer 2.
        assert_eq!(pos(&g, a)[0], 0.0);
        assert_eq!(pos(&g, b)[0], pos(&g, c)[0]); // same layer
        assert!(pos(&g, b)[0] > pos(&g, a)[0]);
        assert!(pos(&g, d)[0] > pos(&g, b)[0]);
        // B and C at different y.
        assert_ne!(pos(&g, b)[1], pos(&g, c)[1]);
    }

    #[test]
    fn longest_path_wins() {
        // source → curve → stamp, source → stamp
        // stamp should be at layer 2 (longest path), not layer 1.
        let mut g = Graph::<TestWireKind>::new();
        let src = g.add_node("src", vec![scalar_out("out1"), scalar_out("out2")], vec![]);
        let curve = g.add_node("curve", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let stamp = g.add_node("stamp", vec![scalar_in("in_a"), scalar_in("in_b")], vec![]);
        wire(&mut g, src, "out1", curve, "in");
        wire(&mut g, curve, "out", stamp, "in_a");
        wire(&mut g, src, "out2", stamp, "in_b");

        g.auto_layout();

        // src at layer 0, curve at layer 1, stamp at layer 2.
        assert_eq!(pos(&g, src)[0], 0.0);
        assert_eq!(pos(&g, curve)[0], SPACING_X);
        assert_eq!(pos(&g, stamp)[0], 2.0 * SPACING_X);
    }

    #[test]
    fn disconnected_nodes() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_in("in")], vec![]);
        let orphan = g.add_node("orphan", vec![scalar_out("out")], vec![]);
        wire(&mut g, a, "out", b, "in");

        g.auto_layout();

        // Orphan placed after the connected subgraph.
        assert!(pos(&g, orphan)[0] > pos(&g, b)[0]);
    }

    #[test]
    fn all_disconnected() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_out("out")], vec![]);
        let c = g.add_node("c", vec![scalar_out("out")], vec![]);

        g.auto_layout();

        // All in a single column at x=0.
        assert_eq!(pos(&g, a)[0], 0.0);
        assert_eq!(pos(&g, b)[0], 0.0);
        assert_eq!(pos(&g, c)[0], 0.0);
        // Different y values.
        let ys: HashSet<i32> = [a, b, c].iter().map(|&id| pos(&g, id)[1] as i32).collect();
        assert_eq!(ys.len(), 3);
    }

    #[test]
    fn no_crossings_simple() {
        // Two independent chains feeding a common sink:
        //   a1 → b1 ─┐
        //   a2 → b2 ─┤→ sink
        //
        // After layout, b1 and b2 should maintain their relative ordering
        // from their sources, so wires don't cross.
        let mut g = Graph::<TestWireKind>::new();
        let a1 = g.add_node("a1", vec![scalar_out("out")], vec![]);
        let a2 = g.add_node("a2", vec![scalar_out("out")], vec![]);
        let b1 = g.add_node("b1", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let b2 = g.add_node("b2", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let sink = g.add_node("sink", vec![scalar_in("in_a"), scalar_in("in_b")], vec![]);
        wire(&mut g, a1, "out", b1, "in");
        wire(&mut g, a2, "out", b2, "in");
        wire(&mut g, b1, "out", sink, "in_a");
        wire(&mut g, b2, "out", sink, "in_b");

        g.auto_layout();

        // If a1 is above a2, then b1 should be above b2 (no crossing).
        let a1_above = pos(&g, a1)[1] < pos(&g, a2)[1];
        let b1_above = pos(&g, b1)[1] < pos(&g, b2)[1];
        assert_eq!(a1_above, b1_above, "barycenter should prevent crossings");
    }

    #[test]
    fn tall_nodes_dont_overlap() {
        // A node with many ports should not overlap its neighbors.
        let mut g = Graph::<TestWireKind>::new();
        // "tall" has 10 output ports.
        let tall = g.add_node(
            "tall",
            (0..10).map(|i| scalar_out(&format!("out{i}"))).collect(),
            vec![],
        );
        let small = g.add_node("small", vec![scalar_out("out")], vec![]);
        let sink = g.add_node("sink", vec![scalar_in("in1"), scalar_in("in2")], vec![]);
        wire(&mut g, tall, "out0", sink, "in1");
        wire(&mut g, small, "out", sink, "in2");

        g.auto_layout();

        // Tall node height ≈ 24 + 8 + 10*18 = 212px.
        // The bottom of one node must be above the top of the next.
        let tall_y = pos(&g, tall)[1];
        let small_y = pos(&g, small)[1];
        let (upper_y, upper_h) = if tall_y < small_y {
            (tall_y, NODE_HEADER_H + BODY_PAD * 2.0 + 10.0 * PORT_ROW_H)
        } else {
            (small_y, NODE_HEADER_H + BODY_PAD * 2.0 + PORT_ROW_H)
        };
        let lower_y = tall_y.max(small_y);
        assert!(
            upper_y + upper_h + GAP_Y <= lower_y + 0.01,
            "nodes overlap: upper ends at {}, lower starts at {}, gap={}",
            upper_y + upper_h,
            lower_y,
            GAP_Y,
        );
    }

    #[test]
    fn multi_port_connections() {
        // Source connects to sink via two separate ports.
        // This must not break the topo sort (in-degree must be counted
        // at the node level, not the connection level).
        let mut g = Graph::<TestWireKind>::new();
        let src = g.add_node("src", vec![scalar_out("out1"), scalar_out("out2")], vec![]);
        let mid = g.add_node(
            "mid",
            vec![scalar_in("in1"), scalar_in("in2"), scalar_out("out")],
            vec![],
        );
        let sink = g.add_node("sink", vec![scalar_in("in")], vec![]);
        wire(&mut g, src, "out1", mid, "in1");
        wire(&mut g, src, "out2", mid, "in2");
        wire(&mut g, mid, "out", sink, "in");

        g.auto_layout();

        // All three nodes should be in distinct layers, not dumped
        // into the disconnected bucket.
        assert_eq!(pos(&g, src)[0], 0.0);
        assert_eq!(pos(&g, mid)[0], SPACING_X);
        assert_eq!(pos(&g, sink)[0], 2.0 * SPACING_X);
    }
}
