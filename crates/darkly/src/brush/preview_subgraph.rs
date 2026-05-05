//! Subgraph builder for per-node previews.
//!
//! Given an active brush graph and a target node id, produces a new graph
//! containing just the target + its transitive `Texture`-relevant
//! dependencies + a synthesised internal `preview_terminal` connected to the
//! target's first `Texture` output. The result is a self-contained graph
//! that, when run through `BrushPreviewRenderer.render_stroke`, produces a
//! thumbnail of what the target node currently outputs.
//!
//! Returns `None` if the target node doesn't exist or has no `Texture`
//! output port — those cases shouldn't render a preview at all.
//!
//! Design rationale lives in
//! [`/home/groot/.claude/plans/humming-wishing-glade.md`] (or its successor in
//! the project's design notes): we synthesise a fresh subgraph per request
//! instead of teaching the runner to evaluate a single node, which would
//! require keeping a specific output texture alive past the dab pool's
//! release-all and adding a new dispatch path. Cloning + pruning is a few
//! lines and reuses 100% of the existing preview render path.

use std::collections::HashSet;

use super::wire::BrushWireType;
use crate::nodegraph::{Graph, NodeId, PortDir, PortRef};

/// Build a self-contained preview graph rooted at `target`.
///
/// - Computes the transitive set of predecessor nodes feeding into `target`.
/// - Clones the active graph, drops every other node (which also drops their
///   connections via `Graph::remove_node`).
/// - Appends a `preview_terminal` node and wires `target`'s first `Texture`
///   output into its `texture` input.
/// - Calls `apply_preview_overrides` to neutralise exposed-scrub ports
///   (`stamp.size`, etc.), keeping the preview consistent with the brush
///   dab thumbnail's "show the brush identity, not working values" rule.
///
/// Returns `None` if `target` doesn't exist or has no `Texture` output.
pub fn build_node_preview_graph(
    active: &Graph<BrushWireType>,
    target: NodeId,
) -> Option<Graph<BrushWireType>> {
    let target_node = active.nodes.get(&target)?;

    // Find the target's first Texture output port.
    let texture_out_name = target_node
        .ports
        .iter()
        .find(|p| p.dir == PortDir::Output && p.wire_type == BrushWireType::Texture)
        .map(|p| p.name.clone())?;

    // Compute the closure of predecessors via reverse-BFS over connections.
    let mut keep: HashSet<NodeId> = HashSet::new();
    keep.insert(target);
    let mut frontier = vec![target];
    while let Some(node_id) = frontier.pop() {
        for conn in active.inputs_for(node_id) {
            if keep.insert(conn.from.node) {
                frontier.push(conn.from.node);
            }
        }
    }

    // Clone the graph and drop every node not in `keep`. `remove_node` also
    // strips the orphaned connections, so we don't need a second pass.
    let mut sub = active.clone();
    let to_drop: Vec<NodeId> = sub
        .nodes
        .keys()
        .copied()
        .filter(|id| !keep.contains(id))
        .collect();
    for id in to_drop {
        // `remove_node` only fails on NodeNotFound; we just enumerated
        // sub.nodes, so the unwrap is safe.
        let _ = sub.remove_node(id);
    }

    // Neutralise exposed-scrub values so preview output reflects the brush's
    // identity, not the user's working size/opacity. Same rule the brush dab
    // thumbnail applies via the engine's `reset_exposed_scrubs` path.
    sub.apply_preview_overrides();

    // Append the synthesised preview terminal. Its `register()` defines a
    // single `texture` input — connect target.<first texture out> → here.
    let registry = super::BrushNodeRegistry::new();
    let term_reg = registry
        .get("preview_terminal")
        .expect("preview_terminal is registered in default_evaluators")
        .clone();
    let term_id = sub.add_node(
        term_reg.type_id,
        term_reg.ports,
        term_reg.params.iter().map(|p| p.default_value()).collect(),
    );
    sub.connect(
        PortRef {
            node: target,
            port: texture_out_name,
        },
        PortRef {
            node: term_id,
            port: "texture".into(),
        },
    )
    .ok()?;

    Some(sub)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::BrushNodeRegistry;

    fn add(
        graph: &mut Graph<BrushWireType>,
        registry: &BrushNodeRegistry,
        type_id: &str,
    ) -> NodeId {
        let reg = registry.get(type_id).unwrap();
        graph.add_node(
            type_id,
            reg.ports.clone(),
            reg.params.iter().map(|p| p.default_value()).collect(),
        )
    }

    /// Targeting a node with a Texture output produces a subgraph that
    /// includes that node, its texture-feeding predecessors, and a
    /// preview_terminal hooked up.
    #[test]
    fn includes_predecessors_and_appends_terminal() {
        let registry = BrushNodeRegistry::new();
        let mut graph = Graph::new();
        let circle = add(&mut graph, &registry, "circle");
        let stamp = add(&mut graph, &registry, "stamp");
        graph
            .connect(
                PortRef {
                    node: circle,
                    port: "texture".into(),
                },
                PortRef {
                    node: stamp,
                    port: "tip".into(),
                },
            )
            .unwrap();

        // Target the stamp node — circle should be pulled in as a dependency.
        let sub = build_node_preview_graph(&graph, stamp).expect("stamp has a Texture output");

        // We expect: stamp + circle + preview_terminal = 3 nodes.
        assert_eq!(sub.nodes.len(), 3);
        assert!(sub.nodes.contains_key(&stamp));
        assert!(sub.nodes.contains_key(&circle));
        let term = sub.nodes.values().find(|n| n.type_id == "preview_terminal");
        assert!(term.is_some(), "preview_terminal should be appended");
        // Stamp.dab → preview_terminal.texture (stamp's first texture output is `dab`).
        let term_id = term.unwrap().id;
        let wired = sub
            .connections
            .iter()
            .any(|c| c.from.node == stamp && c.to.node == term_id && c.to.port == "texture");
        assert!(
            wired,
            "stamp's first Texture output should be wired into preview_terminal.texture",
        );
    }

    /// Targeting a node without a Texture output (e.g. `random` is scalar-only)
    /// returns None — the preview pipeline should not render anything.
    #[test]
    fn returns_none_for_node_without_texture_output() {
        let registry = BrushNodeRegistry::new();
        let mut graph = Graph::new();
        let random = add(&mut graph, &registry, "random");
        assert!(build_node_preview_graph(&graph, random).is_none());
    }

    /// Targeting a node that doesn't exist returns None.
    #[test]
    fn returns_none_for_missing_node() {
        let graph: Graph<BrushWireType> = Graph::new();
        assert!(build_node_preview_graph(&graph, NodeId(42)).is_none());
    }

    /// Unrelated nodes (not upstream of the target) are pruned from the
    /// subgraph. Otherwise the preview render would unnecessarily evaluate
    /// nodes whose output the target doesn't consume.
    #[test]
    fn prunes_unrelated_nodes() {
        let registry = BrushNodeRegistry::new();
        let mut graph = Graph::new();
        let circle = add(&mut graph, &registry, "circle");
        let _unrelated = add(&mut graph, &registry, "circle"); // not connected to anything

        let sub = build_node_preview_graph(&graph, circle).expect("circle has a Texture output");
        // Only target circle + preview_terminal. Unrelated circle is pruned.
        assert_eq!(sub.nodes.len(), 2);
        assert!(sub.nodes.contains_key(&circle));
    }
}
