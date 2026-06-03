//! Portable, human-friendly text representation of a brush graph.
//!
//! `PortableBrush` is the on-wire shape behind the brush builder's
//! Copy/Paste-to-clipboard buttons and the on-disk shape of every
//! built-in brush under `crates/darkly/brushes/*.yaml`. The format is
//! reversible: any brush in memory survives a round trip through
//! `from_brush` → YAML → `into_brush`.
//!
//! Compared to the raw `Graph<W>` JSON, this representation drops
//! everything that can be re-derived from the node registration — port
//! definitions, registration metadata, monotonic id counters — and
//! presents params by name instead of position. The result is something
//! a human can read and an AI can describe.
//!
//! Round-trip rules:
//! - Node ids are plain integers. Import assigns fresh internal ids and
//!   translates the YAML's connection list through a small id map, so
//!   contiguous ids (`1..=N`) round-trip byte-identically while gaps in
//!   hand-edited YAML compact on the next export.
//! - Params are keyed by name; type is coerced from the registration's
//!   `ParamDef`.
//! - `port_defaults` is a diff against the registration's port defaults
//!   — only overridden values appear in YAML. Keeps the format compact
//!   given that brushes typically override 1-3 ports out of 10+.
//! - `exposed` is a *complete* list of the brush's exposed input ports
//!   per node. Declarative ("these are exposed") rather than diff
//!   ("these flip from registration") so it stays readable without
//!   cross-referencing the registration.
//! - Node positions are not stored; auto-layout reflows on import.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::brush::bundle::{Brush, BrushMetadata};
use crate::brush::stabilizer::StabilizerConfig;
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::gpu::params::{ParamValue, PortableValue};
use crate::nodegraph::{Graph, NodeId, PortDir, PortRef};

/// Portable, YAML-friendly snapshot of a brush. Top-level metadata is
/// optional — present for full brushes, omitted for graph-only snippets
/// copied out of the brush builder.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBrush {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stabilizer: Option<StabilizerConfig>,

    pub nodes: BTreeMap<u64, PortableNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<PortableConnection>,
}

/// A single node entry in the portable form.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableNode {
    #[serde(rename = "type")]
    pub type_id: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, PortableValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub port_defaults: BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposed: Vec<String>,
}

/// A wire serialized as `"<from_id>.<from_port> -> <to_id>.<to_port>"`.
/// One line per wire makes the connections list scannable for both
/// humans and AIs — and shorter than any nested tuple form YAML can
/// emit. Round-trips through `Display`/`FromStr`.
#[derive(Clone, Debug, PartialEq)]
pub struct PortableConnection {
    pub from_node: u64,
    pub from_port: String,
    pub to_node: u64,
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
        let parse_side = |side: &str, label: &str| -> Result<(u64, String), String> {
            let (id, port) = side
                .trim()
                .split_once('.')
                .ok_or_else(|| format!("connection '{s}': {label} side must be 'id.port'"))?;
            let id: u64 = id
                .trim()
                .parse()
                .map_err(|e| format!("connection '{s}': {label} id: {e}"))?;
            Ok((id, port.trim().to_string()))
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
            category: brush.metadata.category.clone(),
            description: brush.metadata.description.clone(),
            author: brush.metadata.author.clone(),
            tags: brush.metadata.tags.clone(),
            stabilizer,
            ..Self::from_graph_only(&brush.metadata.graph, registry)?
        })
    }

    /// Build the portable form from a bare graph — no envelope.
    ///
    /// Fails if any node in the graph has a type missing from the
    /// registry. Silently emitting a param-less stub would produce YAML
    /// that always errors on reimport — better to fail at export.
    pub fn from_graph_only(
        graph: &Graph<BrushWireType>,
        registry: &BrushNodeRegistry,
    ) -> Result<Self, String> {
        let mut nodes = BTreeMap::new();
        for (id, node) in graph.nodes() {
            let reg = registry.get(&node.type_id).ok_or_else(|| {
                format!(
                    "node {} has unknown type '{}' — cannot serialize",
                    id.0, node.type_id
                )
            })?;

            // Params: by-name map, only emit entries whose value differs
            // from the registration default. Missing entries on import
            // fall back to the registration default, so this stays a
            // proper diff.
            let mut params = BTreeMap::new();
            for (i, def) in reg.params.iter().enumerate() {
                let Some(value) = node.params.get(i) else {
                    continue;
                };
                if *value == def.default_value() {
                    continue;
                }
                params.insert(def.name().to_string(), PortableValue::from_param(value));
            }

            // Port defaults: diff against registration values. Walk the
            // instance's input ports because that's where the live
            // values are; cross-reference the registration to know
            // which to drop.
            let mut port_defaults = BTreeMap::new();
            // Exposed: complete list of input ports flagged exposed on
            // the instance. Declarative, no registration lookup needed
            // on import.
            let mut exposed = Vec::new();
            for port in &node.ports {
                if port.dir != PortDir::Input {
                    continue;
                }
                let reg_default = reg
                    .ports
                    .iter()
                    .find(|p| p.name == port.name)
                    .map(|p| p.default);
                if Some(port.default) != reg_default {
                    port_defaults.insert(port.name.clone(), port.default);
                }
                if port.exposed {
                    exposed.push(port.name.clone());
                }
            }
            exposed.sort();

            nodes.insert(
                id.0,
                PortableNode {
                    type_id: node.type_id.clone(),
                    params,
                    port_defaults,
                    exposed,
                },
            );
        }

        let mut connections: Vec<PortableConnection> = graph
            .connections
            .iter()
            .map(|c| PortableConnection {
                from_node: c.from.node.0,
                from_port: c.from.port.clone(),
                to_node: c.to.node.0,
                to_port: c.to.port.clone(),
            })
            .collect();
        // Sort so identical graphs serialize to byte-identical YAML.
        connections.sort_by(|a, b| {
            (a.from_node, &a.from_port, a.to_node, &a.to_port).cmp(&(
                b.from_node,
                &b.from_port,
                b.to_node,
                &b.to_port,
            ))
        });

        Ok(Self {
            nodes,
            connections,
            ..Self::default()
        })
    }

    /// Materialize a full `Brush` from the portable form. Re-derives port
    /// shapes from the registration and validates the graph compiles.
    pub fn into_brush(self, registry: &BrushNodeRegistry) -> Result<Brush, String> {
        let graph = self.graph_from_nodes(registry)?;
        crate::brush::compile_graph(&graph)?;
        let mut metadata = BrushMetadata::from_graph(self.name, graph);
        metadata.category = self.category;
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
        // fresh internal ids and translate the connection list through
        // this map. Iterating the BTreeMap in sorted order keeps the
        // assignment deterministic, so contiguous YAML ids round-trip
        // byte-identically.
        let mut id_map: BTreeMap<u64, NodeId> = BTreeMap::new();
        for (&yaml_id, pn) in &self.nodes {
            let reg = registry
                .get(&pn.type_id)
                .ok_or_else(|| format!("unknown node type '{}'", pn.type_id))?;

            // Params: positional vec, defaulted from registration, then
            // overridden by name from the YAML. One pass — both the
            // existence check and the index lookup come from the same
            // find.
            let mut params: Vec<ParamValue> =
                reg.params.iter().map(|d| d.default_value()).collect();
            for (name, value) in &pn.params {
                let Some((idx, def)) = reg
                    .params
                    .iter()
                    .enumerate()
                    .find(|(_, d)| d.name() == name)
                else {
                    return Err(format!("unknown param '{name}' on '{}'", pn.type_id));
                };
                params[idx] = def.coerce_portable(value.clone()).map_err(|m| {
                    format!(
                        "param '{name}' on '{}': expected {}, got {}",
                        pn.type_id, m.expected, m.actual
                    )
                })?;
            }

            // Ports: clone from registration, then apply port_defaults
            // and exposed overrides.
            let mut ports = reg.ports.clone();
            for (name, &value) in &pn.port_defaults {
                let port = ports
                    .iter_mut()
                    .find(|p| p.name == *name && p.dir == PortDir::Input)
                    .ok_or_else(|| {
                        format!(
                            "unknown input port '{name}' on '{}' (port_defaults)",
                            pn.type_id
                        )
                    })?;
                port.default = value;
            }
            // Exposed list is declarative: every input port's `exposed`
            // flag is reset, then ports named in the list are set true.
            for port in ports.iter_mut() {
                if port.dir == PortDir::Input {
                    port.exposed = false;
                }
            }
            for name in &pn.exposed {
                let port = ports
                    .iter_mut()
                    .find(|p| p.name == *name && p.dir == PortDir::Input)
                    .ok_or_else(|| {
                        format!("unknown input port '{name}' on '{}' (exposed)", pn.type_id)
                    })?;
                port.exposed = true;
            }

            let new_id = graph.add_node(pn.type_id.clone(), ports, params);
            id_map.insert(yaml_id, new_id);
        }

        for c in &self.connections {
            let from = *id_map
                .get(&c.from_node)
                .ok_or_else(|| format!("connection '{c}': unknown node id {}", c.from_node))?;
            let to = *id_map
                .get(&c.to_node)
                .ok_or_else(|| format!("connection '{c}': unknown node id {}", c.to_node))?;
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
            (a.from.node.0, &a.from.port, a.to.node.0, &a.to.port).cmp(&(
                b.from.node.0,
                &b.from.port,
                b.to.node.0,
                &b.to.port,
            ))
        });
        Ok(graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::registry;

    /// Round-trip the default graph and confirm the result compiles to
    /// the same shape (nodes, connections, params, port defaults,
    /// exposed flags) as the original. This is the headline guarantee
    /// of the portable form. The default graph has unique type_ids per
    /// node, so we match originals to restorations by type_id rather
    /// than by NodeId — the round trip is not required to preserve
    /// internal ids, only topology.
    #[test]
    fn default_graph_round_trip() {
        let registry = registry();
        let graph = crate::brush::default_graph();
        let portable = PortableBrush::from_graph_only(&graph, registry).expect("serialize");
        let yaml = serde_yml::to_string(&portable).expect("yaml");
        let parsed: PortableBrush = serde_yml::from_str(&yaml).expect("parse");
        let restored = parsed.into_graph(registry).expect("import");

        assert_eq!(graph.nodes().len(), restored.nodes().len());
        assert_eq!(graph.connections.len(), restored.connections.len());

        for original in graph.nodes().values() {
            let restored_node = restored
                .nodes()
                .values()
                .find(|n| n.type_id == original.type_id)
                .unwrap_or_else(|| panic!("missing node of type '{}'", original.type_id));
            assert_eq!(original.params, restored_node.params);
            for port in &original.ports {
                if port.dir != PortDir::Input {
                    continue;
                }
                let r = restored_node
                    .ports
                    .iter()
                    .find(|p| p.name == port.name)
                    .expect("missing input port");
                assert!(
                    (port.default - r.default).abs() < 1e-6,
                    "default mismatch on {}.{}: {} vs {}",
                    original.type_id,
                    port.name,
                    port.default,
                    r.default
                );
                assert_eq!(
                    port.exposed, r.exposed,
                    "exposed mismatch on {}.{}",
                    original.type_id, port.name
                );
            }
        }
    }

    /// Port-default overrides and exposed flags must survive a round
    /// trip — the round trip is reversible if and only if the per-port
    /// state encoded in `port_defaults` and `exposed` returns intact.
    #[test]
    fn port_overrides_survive() {
        let registry = registry();
        let mut graph = crate::brush::default_graph();
        let circle = *graph
            .nodes()
            .iter()
            .find(|(_, n)| n.type_id == "circle")
            .map(|(id, _)| id)
            .expect("default has a circle node");
        graph.set_port_default(circle, "softness", 0.37).unwrap();
        graph.set_port_exposed(circle, "softness", true).unwrap();

        let portable = PortableBrush::from_graph_only(&graph, registry).unwrap();
        let yaml = serde_yml::to_string(&portable).unwrap();
        let restored = serde_yml::from_str::<PortableBrush>(&yaml)
            .unwrap()
            .into_graph(registry)
            .unwrap();
        let restored_circle = restored
            .nodes()
            .values()
            .find(|n| n.type_id == "circle")
            .expect("restored graph has a circle node");
        let port = restored_circle
            .ports
            .iter()
            .find(|p| p.name == "softness")
            .unwrap();
        assert!((port.default - 0.37).abs() < 1e-6);
        assert!(port.exposed);
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
        let portable: PortableBrush = serde_yml::from_str(yaml).unwrap();
        let err = portable.into_graph(registry).unwrap_err();
        assert!(err.contains("unknown node type"), "got: {err}");
    }

    /// Unknown param name must be rejected loudly — the format is
    /// small enough that silently dropping fields would hide bugs in
    /// hand-edited YAML and built-in brush files.
    #[test]
    fn unknown_param_rejected() {
        let registry = registry();
        let yaml = "\
nodes:
  1:
    type: circle
    params:
      this_param_does_not_exist: 1
";
        let portable: PortableBrush = serde_yml::from_str(yaml).unwrap();
        let err = portable.into_graph(registry).unwrap_err();
        assert!(err.contains("unknown param"), "got: {err}");
    }

    /// Param type-mismatch must be rejected — passing a curve into an
    /// integer slot should fail with a clear error.
    #[test]
    fn param_type_mismatch_rejected() {
        let registry = registry();
        let yaml = "\
nodes:
  1:
    type: circle
    params:
      algorithm: [[0.0, 0.0], [1.0, 1.0]]
";
        let portable: PortableBrush = serde_yml::from_str(yaml).unwrap();
        let err = portable.into_graph(registry).unwrap_err();
        assert!(err.contains("expected integer"), "got: {err}");
    }

    /// Unknown top-level keys must be rejected — `deny_unknown_fields`
    /// keeps typos from being absorbed silently.
    #[test]
    fn unknown_top_level_field_rejected() {
        let yaml = "\
name: Test
made_up_field: 1
nodes: {}
";
        let err = serde_yml::from_str::<PortableBrush>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "got: {err}");
    }

    /// The stabilizer envelope round-trips when set to a non-default
    /// algorithm, and is elided from the YAML when default.
    #[test]
    fn stabilizer_round_trip_and_elision() {
        let registry = registry();
        let mut brush = Brush::from_metadata(BrushMetadata::from_graph(
            "Test",
            crate::brush::default_graph(),
        ));

        // Default stabilizer → elided from YAML.
        let portable = PortableBrush::from_brush(&brush, registry).unwrap();
        let yaml = serde_yml::to_string(&portable).unwrap();
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
        let yaml = serde_yml::to_string(&portable).unwrap();
        let parsed: PortableBrush = serde_yml::from_str(&yaml).unwrap();
        let restored = parsed.into_brush(registry).unwrap();
        assert_eq!(restored.metadata.stabilizer.algorithm, "laplacian");
        assert_eq!(restored.metadata.stabilizer.params.len(), 1);
    }
}
