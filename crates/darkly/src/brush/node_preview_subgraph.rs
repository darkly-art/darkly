//! Subgraph builder for per-node previews.
//!
//! Given an active brush graph and a target node id, produces a fresh,
//! self-contained graph containing the target, its transitive predecessors,
//! and a synthesised terminal chain wired to the target's renderable output.
//! Run through [`crate::brush::preview_renderer::BrushStrokePreviewRenderer`]
//! it produces a thumbnail of what the target node currently outputs.
//!
//! Previewability is declared, not enumerated: a node previews iff one of its
//! output ports is flagged [`PortDef::preview_image`] — a spatial coverage or
//! colour field (see [`BrushNodeRegistration::preview_output`]). A new node
//! type opts into a preview by flagging its image output; there is no per-type
//! allowlist here or on the frontend. Per-dab constants and sensor/math
//! outputs (`random.value`, `paint_color.color`) leave the flag off — they'd
//! render as flat blobs, not images.
//!
//! The output is wired into the terminal by **wire type**, never node identity:
//!
//! - `Vec4` (a color field — `image.color`, `noise.color`, `stamp.dab`) →
//!   straight into `paint.rgba`. The dab shows the color.
//! - `Scalar` (a coverage mask — `circle.mask`) → through the canonical
//!   `paint_color → stamp.color`, `mask → stamp.tip`, `stamp.dab → paint.rgba`
//!   chain, so the mask reads as a silhouette in the preview's foreground
//!   colour — exactly as the brush's own dab thumbnail renders it.
//!
//! This is the WGSL-era successor to the pre-`ff5a2eb` `preview_subgraph.rs`,
//! which wired a synthesised `preview_terminal` to the target's first
//! `Texture` output — a wire type the WGSL overhaul deleted.

use std::collections::HashSet;

use super::wire::BrushWireType;
use super::BrushNodeRegistry;
use crate::nodegraph::{Graph, NodeId, PortRef};

/// Build a self-contained preview graph rooted at `target`.
///
/// Returns `None` if `target` doesn't exist or has no renderable output port
/// — those nodes shouldn't render a preview at all.
pub fn build_node_preview_graph(
    active: &Graph<BrushWireType>,
    target: &NodeId,
) -> Option<Graph<BrushWireType>> {
    let registry = super::registry();
    build_with_registry(active, target, registry)
}

/// [`build_node_preview_graph`] with the registry passed explicitly — the
/// seam the unit tests drive.
fn build_with_registry(
    active: &Graph<BrushWireType>,
    target: &NodeId,
    registry: &BrushNodeRegistry,
) -> Option<Graph<BrushWireType>> {
    let target_node = active.nodes().get(target)?;
    // Ask the target's registration which output to visualise. `None` → the
    // node has nothing renderable, so there is no preview.
    let preview_port = registry.get(&target_node.type_id)?.preview_output()?;
    let out_name = preview_port.name.clone();
    let out_type = preview_port.wire_type;

    // Transitive predecessor closure via reverse-BFS over connections.
    let mut keep: HashSet<NodeId> = HashSet::new();
    keep.insert(target.clone());
    let mut frontier = vec![target.clone()];
    while let Some(node_id) = frontier.pop() {
        for conn in active.inputs_for(&node_id) {
            if keep.insert(conn.from.node.clone()) {
                frontier.push(conn.from.node.clone());
            }
        }
    }

    // Clone the graph and drop every node not upstream of the target.
    // `remove_node` also strips the orphaned connections and exposed-port
    // entries, so a single pass suffices.
    let mut sub = active.clone();
    let to_drop: Vec<NodeId> = sub
        .nodes()
        .keys()
        .filter(|id| !keep.contains(*id))
        .cloned()
        .collect();
    for id in to_drop {
        let _ = sub.remove_node(&id);
    }

    // Synthesise the terminal chain. A fresh `pen_input` supplies the dab
    // position; the target's output routes to `paint.rgba` by wire type.
    let ports = |type_id: &str| registry.get(type_id).map(|r| r.ports.clone());
    let pen = sub.add_node("pen_input", ports("pen_input")?);
    let paint = sub.add_node("paint", ports("paint")?);
    sub.connect(
        PortRef {
            node: pen,
            port: "position".into(),
        },
        PortRef {
            node: paint.clone(),
            port: "position".into(),
        },
    )
    .ok()?;

    match out_type {
        // A color field fills the dab directly.
        BrushWireType::Vec4 => {
            sub.connect(
                PortRef {
                    node: target.clone(),
                    port: out_name,
                },
                PortRef {
                    node: paint,
                    port: "rgba".into(),
                },
            )
            .ok()?;
        }
        // A coverage mask goes through the canonical stamp chain, tinted by
        // the foreground colour — the same path the brush's dab thumbnail uses.
        BrushWireType::Scalar => {
            let paint_color = sub.add_node("paint_color", ports("paint_color")?);
            let stamp = sub.add_node("stamp", ports("stamp")?);
            sub.connect(
                PortRef {
                    node: paint_color,
                    port: "color".into(),
                },
                PortRef {
                    node: stamp.clone(),
                    port: "color".into(),
                },
            )
            .ok()?;
            sub.connect(
                PortRef {
                    node: target.clone(),
                    port: out_name,
                },
                PortRef {
                    node: stamp.clone(),
                    port: "tip".into(),
                },
            )
            .ok()?;
            sub.connect(
                PortRef {
                    node: stamp,
                    port: "dab".into(),
                },
                PortRef {
                    node: paint,
                    port: "rgba".into(),
                },
            )
            .ok()?;
        }
        // `preview_output` only ever returns a renderable (Scalar | Vec4) port.
        _ => return None,
    }

    Some(sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> &'static BrushNodeRegistry {
        super::super::registry()
    }

    fn add(graph: &mut Graph<BrushWireType>, type_id: &str) -> NodeId {
        let ports = registry().get(type_id).unwrap().ports.clone();
        graph.add_node(type_id, ports)
    }

    fn count_type(graph: &Graph<BrushWireType>, type_id: &str) -> usize {
        graph
            .nodes()
            .values()
            .filter(|n| n.type_id == type_id)
            .count()
    }

    /// Targeting a node with a `Scalar` output pulls in its predecessors and
    /// appends the stamp terminal chain.
    #[test]
    fn scalar_output_includes_predecessors_and_appends_stamp_chain() {
        let mut graph = Graph::new();
        let pen = add(&mut graph, "pen_input");
        let shape = add(&mut graph, "circle");
        // circle.rotation_input ← pen.drawing_angle, so pen is a predecessor.
        graph
            .connect(
                PortRef {
                    node: pen.clone(),
                    port: "drawing_angle".into(),
                },
                PortRef {
                    node: shape.clone(),
                    port: "rotation_input".into(),
                },
            )
            .unwrap();

        let sub = build_node_preview_graph(&graph, &shape).expect("circle has a Scalar output");

        assert!(sub.nodes().contains_key(&shape));
        // The original pen predecessor is kept.
        assert!(sub.nodes().contains_key(&pen));
        // A stamp chain was synthesised: stamp + paint + paint_color + a fresh pen.
        assert_eq!(count_type(&sub, "stamp"), 1);
        assert_eq!(count_type(&sub, "paint"), 1);
        assert_eq!(count_type(&sub, "paint_color"), 1);
        // circle.mask → stamp.tip is wired.
        assert!(sub
            .connections
            .iter()
            .any(|c| c.from.node == shape && c.from.port == "mask" && c.to.port == "tip"));
    }

    /// Targeting a node with a `Vec4` output wires it straight into
    /// `paint.rgba` — no stamp needed.
    #[test]
    fn vec4_output_wires_directly_into_paint() {
        let mut graph = Graph::new();
        let image = add(&mut graph, "image");

        let sub = build_node_preview_graph(&graph, &image).expect("image has a Vec4 output");

        assert_eq!(count_type(&sub, "paint"), 1);
        assert_eq!(count_type(&sub, "stamp"), 0, "Vec4 output needs no stamp");
        assert!(sub
            .connections
            .iter()
            .any(|c| c.from.node == image && c.from.port == "color" && c.to.port == "rgba"));
    }

    /// Targeting a node whose only output is non-renderable (`paint`'s sole
    /// output is `dab_size: Vec2`) returns `None`.
    #[test]
    fn returns_none_for_node_without_renderable_output() {
        let mut graph = Graph::new();
        let paint = add(&mut graph, "paint");
        assert!(build_node_preview_graph(&graph, &paint).is_none());
    }

    /// A missing target returns `None`.
    #[test]
    fn returns_none_for_missing_node() {
        let graph: Graph<BrushWireType> = Graph::new();
        assert!(build_node_preview_graph(&graph, &NodeId("nope".into())).is_none());
    }

    /// `preview_output` reflects each node's declared `preview_image` flag,
    /// not its wire types: nodes with a spatial image output report one;
    /// per-dab constants and sensor/math/terminal nodes do not — even though
    /// `random.value`/`paint_color.color` share the `Scalar`/`Vec4` wire types
    /// with the real image outputs. This is the type-owned gate both the
    /// frontend previewability check and the subgraph builder share.
    #[test]
    fn preview_output_reflects_declared_image_flag() {
        let reg = registry();
        for id in ["circle", "polygon", "image", "noise", "stamp"] {
            assert!(
                reg.get(id).unwrap().preview_output().is_some(),
                "{id} declares a spatial image output and should be previewable",
            );
        }
        // Renderable wire types, but per-dab constants — not spatial images.
        for id in ["random", "paint_color", "multiply", "paint", "pen_input"] {
            assert!(
                reg.get(id).unwrap().preview_output().is_none(),
                "{id} has no spatial image output and must not preview",
            );
        }
    }

    /// Unrelated nodes (not upstream of the target) are pruned.
    #[test]
    fn prunes_unrelated_nodes() {
        let mut graph = Graph::new();
        let shape = add(&mut graph, "circle");
        let _unrelated = add(&mut graph, "noise"); // not connected to anything

        let sub = build_node_preview_graph(&graph, &shape).expect("circle previews");
        assert_eq!(count_type(&sub, "noise"), 0, "unrelated noise is pruned");
        assert!(sub.nodes().contains_key(&shape));
    }
}
