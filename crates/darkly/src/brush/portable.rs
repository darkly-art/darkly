//! Portable, human-friendly text representation of a brush graph.
//!
//! `PortableBrush` is the on-wire shape behind the brush builder's
//! Copy/Paste-to-clipboard buttons and the on-disk shape of every
//! built-in brush under `crates/darkly/brushes/*.yaml`. The format is
//! reversible: any brush in memory survives a round trip through
//! `from_brush` → YAML → `into_brush`.
//!
//! Compared to the raw `Graph<W>` JSON, this representation drops
//! everything that can be re-derived from the node registration (port
//! definitions, registration metadata) and presents input values by name.
//! The result is something a human can read and an AI can describe.
//!
//! Round-trip rules:
//! - Node ids are kind-derived strings: the first node of a kind is its
//!   `type_id` (`"noise"`), the Nth is `"<type_id>_<N>"` (`"noise_2"`).
//!   Import assigns fresh internal ids via `Graph::add_node` and translates
//!   the YAML's connection list through a small id map, so files authored in
//!   this convention round-trip byte-identically. Same-kind disambiguation
//!   follows the `BTreeMap` key order of the source file (lexicographic), so
//!   a hand-edited file using arbitrary same-kind keys normalizes on export.
//! - `inputs` is a single by-name map of every input whose authored value
//!   differs from the node registration's default: one map for every
//!   input kind (scalar default, enum index, texture name, curve points).
//!   Only overrides appear in YAML, so the format stays compact given that
//!   brushes typically override 1-3 inputs out of 10+. Each value is
//!   coerced to the port's wire type on import.
//! - `exposed` is a *complete* list of the brush's exposed input ports
//!   per node. Declarative ("these are exposed") rather than diff
//!   ("these flip from registration") so it stays readable without
//!   cross-referencing the registration.
//! - Node positions are not stored; auto-layout reflows on import.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::brush::input_value::InputValue;
use crate::brush::metadata::{Brush, BrushMetadata};
use crate::brush::stabilizer::StabilizerConfig;
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::gpu::params::PortableValue;
use crate::nodegraph::{exposed_port_key, ExposedPortMeta, Graph, NodeId, PortDir, PortRef};
use indexmap::IndexMap;

/// Portable, YAML-friendly snapshot of a brush. Top-level metadata is
/// optional: present for full brushes, omitted for graph-only snippets
/// copied out of the brush builder.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBrush {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stabilizer: Option<StabilizerConfig>,

    pub nodes: BTreeMap<String, PortableNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<PortableConnection>,
    /// Ordered brush-bar entries. Keys are `"<yaml_node_id>.<port_name>"`
    /// (matching the `nodes` map). On import the yaml id is rewritten to
    /// the freshly-assigned internal `NodeId`. Order is the brush-bar
    /// display order: `IndexMap` preserves it through every YAML/JSON
    /// round trip.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub exposed_ports: IndexMap<String, ExposedPortMeta>,
}

/// A single node entry in the portable form.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableNode {
    #[serde(rename = "type")]
    pub type_id: String,
    /// Free-form author annotation on this node. Empty is elided so
    /// un-annotated nodes stay a bare `type`/`inputs` entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// Input values that differ from the node registration's defaults, keyed
    /// by input name. One unified map: every input kind (scalar default,
    /// enum index, texture name, curve points, …) rides the same
    /// [`PortableValue`] DTO. Only overrides serialize, so the format stays
    /// a compact diff.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, PortableValue>,
    /// Per-input slider-bound overrides, keyed by input name, as
    /// `[min, max]`. Diffed against the registration exactly like `inputs`,
    /// so only genuinely re-ranged ports serialize.
    ///
    /// Where `inputs` authors *where the knob sits*, this authors *how far it
    /// travels*: the escape hatch for a port whose registration range is a
    /// poor fit for one brush. A math node declaring `0..1` can be given a
    /// bipolar `[-1.0, 1.0]` control, or a port whose useful band is a sliver
    /// of its declared range can be narrowed onto it, without a helper node
    /// in the graph doing the arithmetic.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ranges: BTreeMap<String, [f32; 2]>,
}

/// A wire serialized as `"<from_id>.<from_port> -> <to_id>.<to_port>"`.
/// One line per wire makes the connections list scannable for both
/// humans and AIs, and shorter than any nested tuple form YAML can
/// emit. Round-trips through `Display`/`FromStr`.
#[derive(Clone, Debug, PartialEq)]
pub struct PortableConnection {
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
}

impl std::fmt::Display for PortableConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{} -> {}.{}",
            self.from_node, self.from_port, self.to_node, self.to_port
        )
    }
}

impl std::str::FromStr for PortableConnection {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (lhs, rhs) = s.split_once("->").ok_or_else(|| {
            format!("connection '{s}': expected 'from_id.from_port -> to_id.to_port'")
        })?;
        let parse_side = |side: &str, label: &str| -> Result<(String, String), String> {
            let (id, port) = side
                .trim()
                .split_once('.')
                .ok_or_else(|| format!("connection '{s}': {label} side must be 'id.port'"))?;
            Ok((id.trim().to_string(), port.trim().to_string()))
        };
        let (from_node, from_port) = parse_side(lhs, "from")?;
        let (to_node, to_port) = parse_side(rhs, "to")?;
        Ok(Self {
            from_node,
            from_port,
            to_node,
            to_port,
        })
    }
}

impl Serialize for PortableConnection {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PortableConnection {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl PortableBrush {
    /// Build the portable form from a full `Brush` (metadata + graph).
    pub fn from_brush(brush: &Brush, registry: &BrushNodeRegistry) -> Result<Self, String> {
        let stabilizer = (brush.metadata.stabilizer != StabilizerConfig::default())
            .then(|| brush.metadata.stabilizer.clone());
        Ok(Self {
            name: brush.metadata.name.clone(),
            description: brush.metadata.description.clone(),
            author: brush.metadata.author.clone(),
            tags: brush.metadata.tags.clone(),
            stabilizer,
            ..Self::from_graph_only(&brush.metadata.graph, registry)?
        })
    }

    /// Build the portable form from a bare graph, no envelope.
    ///
    /// Fails if any node in the graph has a type missing from the
    /// registry. Silently emitting a param-less stub would produce YAML
    /// that always errors on reimport; better to fail at export.
    pub fn from_graph_only(
        graph: &Graph<BrushWireType>,
        registry: &BrushNodeRegistry,
    ) -> Result<Self, String> {
        let mut nodes = BTreeMap::new();
        for (id, node) in graph.nodes() {
            let reg = registry.get(&node.type_id).ok_or_else(|| {
                format!(
                    "node {} has unknown type '{}', cannot serialize",
                    id.0, node.type_id
                )
            })?;

            // Inputs: one by-name map diffed against the registration. Walk
            // the instance's input ports (where the live values are) and
            // cross-reference the registration to drop unchanged defaults.
            // Missing entries on import fall back to the registration value,
            // so this stays a compact diff over every input kind.
            let mut inputs = BTreeMap::new();
            for port in &node.ports {
                if port.dir != PortDir::Input {
                    continue;
                }
                let reg_value = reg
                    .ports
                    .iter()
                    .find(|p| p.name == port.name)
                    .map(|p| &p.value);
                if reg_value != Some(&port.value) {
                    inputs.insert(port.name.clone(), port.value.to_portable());
                }
            }

            // Slider bounds: the same diff-against-registration treatment,
            // so a brush that never re-ranges anything emits no `ranges` key.
            let mut ranges = BTreeMap::new();
            for port in &node.ports {
                if port.dir != PortDir::Input {
                    continue;
                }
                let Some(reg_port) = reg.ports.iter().find(|p| p.name == port.name) else {
                    continue;
                };
                if reg_port.min != port.min || reg_port.max != port.max {
                    ranges.insert(port.name.clone(), [port.min, port.max]);
                }
            }

            nodes.insert(
                id.0.clone(),
                PortableNode {
                    type_id: node.type_id.clone(),
                    comment: node.comment.clone(),
                    inputs,
                    ranges,
                },
            );
        }

        let mut connections: Vec<PortableConnection> = graph
            .connections
            .iter()
            .map(|c| PortableConnection {
                from_node: c.from.node.0.clone(),
                from_port: c.from.port.clone(),
                to_node: c.to.node.0.clone(),
                to_port: c.to.port.clone(),
            })
            .collect();
        // Sort so identical graphs serialize to byte-identical YAML.
        connections.sort_by(|a, b| {
            (&a.from_node, &a.from_port, &a.to_node, &a.to_port).cmp(&(
                &b.from_node,
                &b.from_port,
                &b.to_node,
                &b.to_port,
            ))
        });

        // Brush-bar entries: graph keys are already `"<NodeId>.<port>"`
        // and the in-memory `id.0` doubles as the yaml id, so the map
        // can be copied wholesale.
        let exposed_ports = graph.exposed_ports.clone();

        Ok(Self {
            nodes,
            connections,
            exposed_ports,
            ..Self::default()
        })
    }

    /// Materialize a full `Brush` from the portable form under the identity
    /// `id`. Re-derives port shapes from the registration and validates the
    /// graph compiles.
    ///
    /// The id is the caller's to supply: the portable form is a graph plus
    /// describing metadata, and which brush it *is* depends on where it came
    /// from: a shipped brush's file stem, or a minted id for one the painter
    /// saved.
    pub fn into_brush(self, registry: &BrushNodeRegistry, id: &str) -> Result<Brush, String> {
        let graph = self.graph_from_nodes(registry)?;
        crate::brush::compile_graph(&graph)?;
        let mut metadata = BrushMetadata::from_graph(id, self.name, graph);
        metadata.description = self.description;
        metadata.author = self.author;
        metadata.tags = self.tags;
        if let Some(s) = self.stabilizer {
            metadata.stabilizer = s;
        }
        Ok(Brush::from_metadata(metadata))
    }

    /// Materialize just the graph (drops envelope). Used by the editor
    /// Paste path where the active brush's metadata is preserved.
    pub fn into_graph(self, registry: &BrushNodeRegistry) -> Result<Graph<BrushWireType>, String> {
        let graph = self.graph_from_nodes(registry)?;
        crate::brush::compile_graph(&graph)?;
        Ok(graph)
    }

    fn graph_from_nodes(
        &self,
        registry: &BrushNodeRegistry,
    ) -> Result<Graph<BrushWireType>, String> {
        let mut graph = Graph::<BrushWireType>::new();

        // YAML ids are local to the file; let `Graph::add_node` assign
        // fresh kind-derived internal ids and translate the connection list
        // through this map. Iterating the BTreeMap in lexicographic key order
        // keeps same-kind disambiguation deterministic, so files authored in
        // the `noise`/`noise_2` convention round-trip byte-identically.
        let mut id_map: BTreeMap<String, NodeId> = BTreeMap::new();
        for (yaml_id, pn) in &self.nodes {
            let reg = registry
                .get(&pn.type_id)
                .ok_or_else(|| format!("unknown node type '{}'", pn.type_id))?;

            // Ports: clone from registration, then apply the by-name input
            // overrides, coercing each portable value to the port's wire type
            // (the brush-side analog of the filter `coerce_portable` path).
            let mut ports = reg.ports.clone();
            for (name, value) in &pn.inputs {
                let port = ports
                    .iter_mut()
                    .find(|p| p.name == *name && p.dir == PortDir::Input)
                    .ok_or_else(|| format!("unknown input '{name}' on '{}'", pn.type_id))?;
                port.value =
                    InputValue::from_portable(port.wire_type, value.clone()).map_err(|m| {
                        format!(
                            "input '{name}' on '{}': expected {}, got {}",
                            pn.type_id, m.expected, m.actual
                        )
                    })?;
            }

            let new_id = graph.add_node(pn.type_id.clone(), ports);
            if !pn.comment.is_empty() {
                graph
                    .set_node_comment(&new_id, pn.comment.clone())
                    .expect("node just added by add_node must exist");
            }
            // Applied through the graph setter rather than onto the cloned
            // ports above so the ascending-and-finite invariant is enforced
            // in one place for every author: yaml, editor, and paste alike.
            for (name, [min, max]) in &pn.ranges {
                graph
                    .set_port_range(&new_id, name, *min, *max)
                    .map_err(|e| format!("range '{name}' on '{}': {e}", pn.type_id))?;
            }
            id_map.insert(yaml_id.clone(), new_id);
        }

        for c in &self.connections {
            let from = id_map
                .get(&c.from_node)
                .ok_or_else(|| format!("connection '{c}': unknown node id {}", c.from_node))?
                .clone();
            let to = id_map
                .get(&c.to_node)
                .ok_or_else(|| format!("connection '{c}': unknown node id {}", c.to_node))?
                .clone();
            graph
                .connect(
                    PortRef {
                        node: from,
                        port: c.from_port.clone(),
                    },
                    PortRef {
                        node: to,
                        port: c.to_port.clone(),
                    },
                )
                .map_err(|e| format!("connection '{c}': {e}"))?;
        }
        // Sort `connections` so the in-memory graph matches the
        // round-trip output regardless of insertion order.
        graph.connections.sort_by(|a, b| {
            (&a.from.node.0, &a.from.port, &a.to.node.0, &a.to.port).cmp(&(
                &b.from.node.0,
                &b.from.port,
                &b.to.node.0,
                &b.to.port,
            ))
        });

        // Brush-bar entries: `add_node` auto-populated `exposed_ports`
        // from each registration's `.exposed()` flag. The portable form
        // is authoritative, so clear that and re-populate from the saved
        // map, rewriting yaml ids to the freshly-assigned NodeIds.
        // Malformed or stale keys are skipped silently, since the import
        // already validated nodes/ports above.
        graph.exposed_ports.clear();
        for (yaml_key, meta) in &self.exposed_ports {
            let Some((yid_str, port_name)) = yaml_key.split_once('.') else {
                continue;
            };
            let Some(new_id) = id_map.get(yid_str) else {
                continue;
            };
            let new_key = exposed_port_key(new_id, port_name);
            graph.exposed_ports.insert(new_key, meta.clone());
        }
        Ok(graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::registry;
    use crate::gpu::params::ParamValue;

    /// Round-trip the default graph and confirm the result compiles to
    /// the same shape (nodes, connections, params, port defaults) as the
    /// original. Brush-bar entries are covered by `port_overrides_survive`.
    #[test]
    fn default_graph_round_trip() {
        let registry = registry();
        let graph = crate::brush::default_graph();
        let portable = PortableBrush::from_graph_only(&graph, registry).expect("serialize");
        let yaml = serde_yaml_ng::to_string(&portable).expect("yaml");
        let parsed: PortableBrush = serde_yaml_ng::from_str(&yaml).expect("parse");
        let restored = parsed.into_graph(registry).expect("import");

        assert_eq!(graph.nodes().len(), restored.nodes().len());
        assert_eq!(graph.connections.len(), restored.connections.len());

        for original in graph.nodes().values() {
            let restored_node = restored
                .nodes()
                .values()
                .find(|n| n.type_id == original.type_id)
                .unwrap_or_else(|| panic!("missing node of type '{}'", original.type_id));
            for port in &original.ports {
                if port.dir != PortDir::Input {
                    continue;
                }
                let r = restored_node
                    .ports
                    .iter()
                    .find(|p| p.name == port.name)
                    .expect("missing input port");
                assert_eq!(
                    port.value, r.value,
                    "value mismatch on {}.{}",
                    original.type_id, port.name
                );
            }
        }
        // Brush-bar entry count matches across the round trip.
        assert_eq!(graph.exposed_ports.len(), restored.exposed_ports.len());
    }

    /// Port-default overrides and brush-bar exposure (with custom meta)
    /// must survive a round trip; the round trip is reversible if and
    /// only if both per-port defaults and the graph-level
    /// `exposed_ports` map return intact.
    #[test]
    fn port_overrides_survive() {
        let registry = registry();
        let mut graph = crate::brush::default_graph();
        let shape = graph
            .nodes()
            .iter()
            .find(|(_, n)| n.type_id == "circle")
            .map(|(id, _)| id.clone())
            .expect("default has a circle node");
        graph.set_port_default(&shape, "softness", 0.37).unwrap();
        graph.expose_port(&shape, "softness").unwrap();
        let shape_key = exposed_port_key(&shape, "softness");
        graph
            .set_exposed_port_meta(
                &shape_key,
                "Softness".into(),
                "Edge falloff".into(),
                "fa6-solid:circle-half-stroke".into(),
            )
            .unwrap();

        let portable = PortableBrush::from_graph_only(&graph, registry).unwrap();
        let yaml = serde_yaml_ng::to_string(&portable).unwrap();
        let restored = serde_yaml_ng::from_str::<PortableBrush>(&yaml)
            .unwrap()
            .into_graph(registry)
            .unwrap();
        let restored_shape = restored
            .nodes()
            .values()
            .find(|n| n.type_id == "circle")
            .expect("restored graph has a circle node");
        let port = restored_shape
            .ports
            .iter()
            .find(|p| p.name == "softness")
            .unwrap();
        assert!((port.value.as_f32() - 0.37).abs() < 1e-6);
        assert!(restored.is_port_exposed(&restored_shape.id, "softness"));
        let restored_key = exposed_port_key(&restored_shape.id, "softness");
        let meta = &restored.exposed_ports[&restored_key];
        assert_eq!(meta.label, "Softness");
        assert_eq!(meta.description, "Edge falloff");
        assert_eq!(meta.icon, "fa6-solid:circle-half-stroke");
    }

    /// A re-ranged port survives the yaml round trip, and a graph that
    /// re-ranges nothing emits no `ranges` key at all: the diff stays a
    /// diff, so untouched brushes' yaml doesn't churn.
    #[test]
    fn port_ranges_survive_and_stay_a_diff() {
        let registry = registry();
        let mut graph = crate::brush::default_graph();
        let circle = graph
            .nodes()
            .iter()
            .find(|(_, n)| n.type_id == "circle")
            .map(|(id, _)| id.clone())
            .expect("default has a circle node");

        // Untouched graph: no node carries a `ranges` entry.
        let clean = PortableBrush::from_graph_only(&graph, registry).unwrap();
        assert!(
            clean.nodes.values().all(|n| n.ranges.is_empty()),
            "unmodified graph must not serialize any ranges"
        );
        assert!(!serde_yaml_ng::to_string(&clean).unwrap().contains("ranges"));

        graph
            .set_port_range(&circle, "softness", -1.0, 2.5)
            .unwrap();

        let portable = PortableBrush::from_graph_only(&graph, registry).unwrap();
        let yaml = serde_yaml_ng::to_string(&portable).unwrap();
        let restored = serde_yaml_ng::from_str::<PortableBrush>(&yaml)
            .unwrap()
            .into_graph(registry)
            .unwrap();
        let port = restored
            .nodes()
            .values()
            .find(|n| n.type_id == "circle")
            .expect("restored graph has a circle node")
            .ports
            .iter()
            .find(|p| p.name == "softness")
            .unwrap();
        assert_eq!((port.min, port.max), (-1.0, 2.5));
    }

    /// A hand-edited yaml carrying a degenerate or inverted range is
    /// rejected at import rather than producing a control whose normalize
    /// and clamp arithmetic silently misbehaves.
    #[test]
    fn inverted_yaml_range_is_rejected_at_import() {
        let registry = registry();
        let graph = crate::brush::default_graph();
        let mut portable = PortableBrush::from_graph_only(&graph, registry).unwrap();
        let circle = portable
            .nodes
            .values_mut()
            .find(|n| n.type_id == "circle")
            .expect("default has a circle node");
        circle.ranges.insert("softness".into(), [1.0, 0.0]);

        let err = portable.into_graph(registry).unwrap_err();
        assert!(err.contains("softness"), "unexpected error: {err}");
    }

    /// The unified model's headline guarantee: a non-wirable input (here
    /// the circle node's `algorithm` Enum) is fully *exposable*, and both the
    /// exposure and a non-default enum value survive a YAML round trip.
    #[test]
    fn enum_input_is_exposable_and_round_trips() {
        let registry = registry();
        let mut graph = crate::brush::default_graph();
        let shape = graph
            .nodes()
            .iter()
            .find(|(_, n)| n.type_id == "circle")
            .map(|(id, _)| id.clone())
            .expect("default has a circle node");

        // Override the enum to Perlin (1) and expose it; neither was
        // possible under the old param system.
        graph
            .set_port_value(&shape, "algorithm", InputValue::Int(1))
            .unwrap();
        graph.expose_port(&shape, "algorithm").unwrap();

        let portable = PortableBrush::from_graph_only(&graph, registry).unwrap();
        let yaml = serde_yaml_ng::to_string(&portable).unwrap();
        let restored = serde_yaml_ng::from_str::<PortableBrush>(&yaml)
            .unwrap()
            .into_graph(registry)
            .unwrap();

        let restored_shape = restored
            .nodes()
            .values()
            .find(|n| n.type_id == "circle")
            .expect("restored graph has a circle node");
        let algo = restored_shape
            .ports
            .iter()
            .find(|p| p.name == "algorithm")
            .unwrap();
        assert_eq!(algo.value, InputValue::Int(1));
        assert!(!algo.wirable, "an enum input must not be wirable");
        assert!(restored.is_port_exposed(&restored_shape.id, "algorithm"));
    }

    /// Unknown node `type:` must fail import with a clear error,
    /// not panic or silently drop the node.
    #[test]
    fn unknown_type_rejected() {
        let registry = registry();
        let yaml = "\
nodes:
  1:
    type: never_was_a_node
";
        let portable: PortableBrush = serde_yaml_ng::from_str(yaml).unwrap();
        let err = portable.into_graph(registry).unwrap_err();
        assert!(err.contains("unknown node type"), "got: {err}");
    }

    /// Unknown input name must be rejected loudly, since the format is
    /// small enough that silently dropping fields would hide bugs in
    /// hand-edited YAML and built-in brush files.
    #[test]
    fn unknown_input_rejected() {
        let registry = registry();
        let yaml = "\
nodes:
  1:
    type: circle
    inputs:
      this_input_does_not_exist: 1
";
        let portable: PortableBrush = serde_yaml_ng::from_str(yaml).unwrap();
        let err = portable.into_graph(registry).unwrap_err();
        assert!(err.contains("unknown input"), "got: {err}");
    }

    /// Input type-mismatch must be rejected: passing a curve into an
    /// enum slot should fail with a clear error.
    #[test]
    fn input_type_mismatch_rejected() {
        let registry = registry();
        let yaml = "\
nodes:
  1:
    type: circle
    inputs:
      algorithm: [[0.0, 0.0], [1.0, 1.0]]
";
        let portable: PortableBrush = serde_yaml_ng::from_str(yaml).unwrap();
        let err = portable.into_graph(registry).unwrap_err();
        assert!(err.contains("expected integer"), "got: {err}");
    }

    /// Unknown top-level keys must be rejected: `deny_unknown_fields`
    /// keeps typos from being absorbed silently.
    #[test]
    fn unknown_top_level_field_rejected() {
        let yaml = "\
name: Test
made_up_field: 1
nodes: {}
";
        let err = serde_yaml_ng::from_str::<PortableBrush>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "got: {err}");
    }

    /// The stabilizer envelope round-trips when set to a non-default
    /// algorithm, and is elided from the YAML when default.
    #[test]
    fn stabilizer_round_trip_and_elision() {
        let registry = registry();
        let mut brush = Brush::from_metadata(BrushMetadata::from_graph(
            "test",
            "Test",
            crate::brush::default_graph(),
        ));

        // Default stabilizer → elided from YAML.
        let portable = PortableBrush::from_brush(&brush, registry).unwrap();
        let yaml = serde_yaml_ng::to_string(&portable).unwrap();
        assert!(
            !yaml.contains("stabilizer"),
            "default stabilizer should be elided\n{yaml}"
        );

        // Non-default → preserved across round trip.
        brush.metadata.stabilizer = StabilizerConfig {
            algorithm: "laplacian".into(),
            params: vec![ParamValue::Float(0.6)],
        };
        let portable = PortableBrush::from_brush(&brush, registry).unwrap();
        let yaml = serde_yaml_ng::to_string(&portable).unwrap();
        let parsed: PortableBrush = serde_yaml_ng::from_str(&yaml).unwrap();
        let restored = parsed.into_brush(registry, "test").unwrap();
        assert_eq!(restored.metadata.stabilizer.algorithm, "laplacian");
        assert_eq!(restored.metadata.stabilizer.params.len(), 1);
    }

    /// Two nodes of the same kind serialize to `random` + `random_2` YAML
    /// keys, re-import to the same two ids, and re-serialize byte-identically,
    /// locking the kind-derived id scheme through a full round trip.
    #[test]
    fn two_random_round_trip() {
        let registry = registry();
        let mut graph = Graph::<BrushWireType>::new();
        let a = graph.add_node("random", registry.get("random").unwrap().ports.clone());
        let b = graph.add_node("random", registry.get("random").unwrap().ports.clone());
        assert_eq!(a, NodeId("random".into()));
        assert_eq!(b, NodeId("random_2".into()));

        let portable = PortableBrush::from_graph_only(&graph, registry).unwrap();
        assert!(portable.nodes.contains_key("random"));
        assert!(portable.nodes.contains_key("random_2"));

        let yaml = serde_yaml_ng::to_string(&portable).unwrap();
        let restored = serde_yaml_ng::from_str::<PortableBrush>(&yaml)
            .unwrap()
            .into_graph(registry)
            .unwrap();
        assert!(restored.nodes().contains_key(&NodeId("random".into())));
        assert!(restored.nodes().contains_key(&NodeId("random_2".into())));

        // Re-serialization is byte-identical.
        let reyaml =
            serde_yaml_ng::to_string(&PortableBrush::from_graph_only(&restored, registry).unwrap())
                .unwrap();
        assert_eq!(yaml, reyaml);
    }

    /// A node comment survives the full YAML round trip (including multi-line
    /// text, which YAML emits as a block scalar), is emitted only for the
    /// node that has one, and re-lands on the correct id after same-kind
    /// normalization.
    #[test]
    fn node_comment_round_trips() {
        let registry = registry();
        let mut graph = Graph::<BrushWireType>::new();
        let a = graph.add_node("random", registry.get("random").unwrap().ports.clone());
        let _b = graph.add_node("random", registry.get("random").unwrap().ports.clone());
        graph
            .set_node_comment(&a, "roughness source\nkeep frequency low".into())
            .unwrap();

        let yaml =
            serde_yaml_ng::to_string(&PortableBrush::from_graph_only(&graph, registry).unwrap())
                .unwrap();
        // Only the annotated node emits `comment:`.
        assert_eq!(yaml.matches("comment:").count(), 1);

        let restored = serde_yaml_ng::from_str::<PortableBrush>(&yaml)
            .unwrap()
            .into_graph(registry)
            .unwrap();
        assert_eq!(
            restored
                .nodes()
                .get(&NodeId("random".into()))
                .unwrap()
                .comment,
            "roughness source\nkeep frequency low"
        );
        assert_eq!(
            restored
                .nodes()
                .get(&NodeId("random_2".into()))
                .unwrap()
                .comment,
            ""
        );
    }

    /// A hand-authored file whose same-kind keys don't follow the `_N`
    /// convention normalizes on import: same-kind disambiguation is
    /// BTreeMap-key-order-dependent, so the lexicographically-first key
    /// (`randomA`) takes the bare `random` id and `randomB` becomes
    /// `random_2`. Connections referencing the file keys still resolve.
    #[test]
    fn same_kind_keys_normalize_by_lexicographic_order() {
        let registry = registry();
        let yaml = "\
nodes:
  randomB:
    type: random
  randomA:
    type: random
";
        let restored = serde_yaml_ng::from_str::<PortableBrush>(yaml)
            .unwrap()
            .into_graph(registry)
            .unwrap();
        // BTreeMap iterates randomA before randomB, so randomA → "random".
        assert!(restored.nodes().contains_key(&NodeId("random".into())));
        assert!(restored.nodes().contains_key(&NodeId("random_2".into())));
    }
}
