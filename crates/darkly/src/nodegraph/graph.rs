use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::WireKind;
use crate::gpu::params::ParamValue;

// ── Identifiers ──────────────────────────────────────────────────────

/// Stable node identity inside a graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

/// Reference to a specific port on a specific node.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortRef {
    pub node: NodeId,
    pub port: String,
}

/// A directed wire between two ports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub from: PortRef,
    pub to: PortRef,
}

// ── Port definitions ─────────────────────────────────────────────────

/// Direction of data flow through a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortDir {
    Input,
    Output,
}

/// Schema for a single port on a node type.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct PortDef<W: WireKind> {
    pub name: String,
    pub dir: PortDir,
    pub wire_type: W,
    /// Slider min when the port is disconnected (UI metadata only).
    pub min: f32,
    /// Slider max when the port is disconnected (UI metadata only).
    pub max: f32,
    /// Default value when the port is disconnected.
    pub default: f32,
}

impl<W: WireKind> PortDef<W> {
    pub fn input(name: impl Into<String>, wire_type: W) -> Self {
        Self {
            name: name.into(),
            dir: PortDir::Input,
            wire_type,
            min: 0.0,
            max: 1.0,
            default: 0.0,
        }
    }

    pub fn output(name: impl Into<String>, wire_type: W) -> Self {
        Self {
            name: name.into(),
            dir: PortDir::Output,
            wire_type,
            min: 0.0,
            max: 1.0,
            default: 0.0,
        }
    }

    pub fn with_range(mut self, min: f32, max: f32, default: f32) -> Self {
        self.min = min;
        self.max = max;
        self.default = default;
        self
    }
}

// ── Node instance ────────────────────────────────────────────────────

/// A placed node in a graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct NodeInstance<W: WireKind> {
    pub id: NodeId,
    /// References the `type_id` from the `NodeRegistration`.
    pub type_id: String,
    /// Port definitions (copied from registration on creation).
    pub ports: Vec<PortDef<W>>,
    /// Per-instance parameter overrides.
    pub params: Vec<ParamValue>,
    /// UI position (for layout persistence).
    pub position: [f32; 2],
}

// ── Errors ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    TypeMismatch {
        from_type: String,
        to_type: String,
    },
    CycleDetected,
    PortNotFound {
        node: NodeId,
        port: String,
    },
    NodeNotFound(NodeId),
    /// An input port may only have one incoming wire.
    InputAlreadyConnected {
        node: NodeId,
        port: String,
    },
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeMismatch { from_type, to_type } => {
                write!(f, "type mismatch: {from_type} → {to_type}")
            }
            Self::CycleDetected => write!(f, "cycle detected"),
            Self::PortNotFound { node, port } => {
                write!(f, "port '{}' not found on node {:?}", port, node)
            }
            Self::NodeNotFound(id) => write!(f, "node {:?} not found", id),
            Self::InputAlreadyConnected { node, port } => {
                write!(f, "input '{}' on {:?} already connected", port, node)
            }
        }
    }
}

impl std::error::Error for GraphError {}

// ── Graph ────────────────────────────────────────────────────────────

/// A directed acyclic graph of nodes connected by typed wires.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Graph<W: WireKind> {
    pub nodes: HashMap<NodeId, NodeInstance<W>>,
    pub connections: Vec<Connection>,
    next_id: u64,
}

impl<W: WireKind> Default for Graph<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: WireKind> Graph<W> {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            connections: Vec::new(),
            next_id: 1,
        }
    }

    /// Add a node and return its assigned id.
    pub fn add_node(
        &mut self,
        type_id: impl Into<String>,
        ports: Vec<PortDef<W>>,
        params: Vec<ParamValue>,
    ) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(
            id,
            NodeInstance {
                id,
                type_id: type_id.into(),
                ports,
                params,
                position: [0.0, 0.0],
            },
        );
        id
    }

    /// Remove a node and all its connections.
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), GraphError> {
        if self.nodes.remove(&id).is_none() {
            return Err(GraphError::NodeNotFound(id));
        }
        self.connections
            .retain(|c| c.from.node != id && c.to.node != id);
        Ok(())
    }

    /// Connect an output port to an input port, checking types and cycles.
    pub fn connect(&mut self, from: PortRef, to: PortRef) -> Result<(), GraphError> {
        // Resolve port defs.
        let from_def = self.find_port(&from, PortDir::Output)?;
        let to_def = self.find_port(&to, PortDir::Input)?;

        // Type check.
        if !W::compatible(from_def, to_def) {
            return Err(GraphError::TypeMismatch {
                from_type: format!("{:?}", from_def),
                to_type: format!("{:?}", to_def),
            });
        }

        // Input-already-connected check.
        if self.connections.iter().any(|c| c.to == to) {
            return Err(GraphError::InputAlreadyConnected {
                node: to.node,
                port: to.port.clone(),
            });
        }

        // Cycle check: would adding from→to create a cycle?
        // A cycle exists iff `from.node` is reachable from `to.node`
        // through existing connections (i.e., to is upstream of from).
        if self.is_reachable(to.node, from.node) {
            return Err(GraphError::CycleDetected);
        }

        self.connections.push(Connection { from, to });
        Ok(())
    }

    /// Disconnect a specific wire.
    pub fn disconnect(&mut self, from: &PortRef, to: &PortRef) {
        self.connections.retain(|c| &c.from != from || &c.to != to);
    }

    /// All connections whose destination is a port on `node_id`.
    pub fn inputs_for(&self, node_id: NodeId) -> impl Iterator<Item = &Connection> {
        self.connections.iter().filter(move |c| c.to.node == node_id)
    }

    /// All connections whose source is a port on `node_id`.
    pub fn outputs_for(&self, node_id: NodeId) -> impl Iterator<Item = &Connection> {
        self.connections
            .iter()
            .filter(move |c| c.from.node == node_id)
    }

    /// Update a node's UI position.
    pub fn set_node_position(&mut self, id: NodeId, pos: [f32; 2]) -> Result<(), GraphError> {
        let node = self.nodes.get_mut(&id).ok_or(GraphError::NodeNotFound(id))?;
        node.position = pos;
        Ok(())
    }

    /// Update a single parameter value on a node.
    pub fn set_param(
        &mut self,
        id: NodeId,
        index: usize,
        value: ParamValue,
    ) -> Result<(), GraphError> {
        let node = self.nodes.get_mut(&id).ok_or(GraphError::NodeNotFound(id))?;
        if index >= node.params.len() {
            return Err(GraphError::PortNotFound {
                node: id,
                port: format!("param[{}]", index),
            });
        }
        node.params[index] = value;
        Ok(())
    }

    // ── helpers ──────────────────────────────────────────────────────

    /// Find the wire type of a port, returning an error if the node or
    /// port doesn't exist or has the wrong direction.
    fn find_port(&self, pr: &PortRef, expected_dir: PortDir) -> Result<W, GraphError> {
        let node = self
            .nodes
            .get(&pr.node)
            .ok_or(GraphError::NodeNotFound(pr.node))?;
        let def = node
            .ports
            .iter()
            .find(|p| p.name == pr.port && p.dir == expected_dir)
            .ok_or_else(|| GraphError::PortNotFound {
                node: pr.node,
                port: pr.port.clone(),
            })?;
        Ok(def.wire_type)
    }

    /// DFS reachability: can we get from `start` to `target` following
    /// existing connection edges (from.node → to.node)?
    fn is_reachable(&self, start: NodeId, target: NodeId) -> bool {
        let mut visited = HashSet::new();
        let mut stack = vec![start];
        while let Some(current) = stack.pop() {
            if current == target {
                return true;
            }
            if !visited.insert(current) {
                continue;
            }
            for conn in &self.connections {
                if conn.from.node == current {
                    stack.push(conn.to.node);
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodegraph::tests::TestWireKind;

    fn scalar_in(name: &str) -> PortDef<TestWireKind> {
        PortDef::input(name, TestWireKind::Scalar)
    }
    fn scalar_out(name: &str) -> PortDef<TestWireKind> {
        PortDef::output(name, TestWireKind::Scalar)
    }
    fn color_out(name: &str) -> PortDef<TestWireKind> {
        PortDef::output(name, TestWireKind::Color)
    }

    #[test]
    fn add_connect_disconnect_remove() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("source", vec![scalar_out("out")], vec![]);
        let b = g.add_node("sink", vec![scalar_in("in")], vec![]);

        let from = PortRef {
            node: a,
            port: "out".into(),
        };
        let to = PortRef {
            node: b,
            port: "in".into(),
        };

        g.connect(from.clone(), to.clone()).unwrap();
        assert_eq!(g.connections.len(), 1);

        g.disconnect(&from, &to);
        assert_eq!(g.connections.len(), 0);

        g.remove_node(a).unwrap();
        assert!(!g.nodes.contains_key(&a));
    }

    #[test]
    fn cycle_detection() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node(
            "a",
            vec![scalar_in("in"), scalar_out("out")],
            vec![],
        );
        let b = g.add_node(
            "b",
            vec![scalar_in("in"), scalar_out("out")],
            vec![],
        );

        g.connect(
            PortRef { node: a, port: "out".into() },
            PortRef { node: b, port: "in".into() },
        )
        .unwrap();

        let err = g
            .connect(
                PortRef { node: b, port: "out".into() },
                PortRef { node: a, port: "in".into() },
            )
            .unwrap_err();

        assert_eq!(err, GraphError::CycleDetected);
    }

    #[test]
    fn type_mismatch() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![color_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_in("in")], vec![]);

        let err = g
            .connect(
                PortRef { node: a, port: "out".into() },
                PortRef { node: b, port: "in".into() },
            )
            .unwrap_err();

        matches!(err, GraphError::TypeMismatch { .. });
    }

    #[test]
    fn input_already_connected() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_out("out")], vec![]);
        let c = g.add_node("c", vec![scalar_in("in")], vec![]);

        g.connect(
            PortRef { node: a, port: "out".into() },
            PortRef { node: c, port: "in".into() },
        )
        .unwrap();

        let err = g
            .connect(
                PortRef { node: b, port: "out".into() },
                PortRef { node: c, port: "in".into() },
            )
            .unwrap_err();

        matches!(err, GraphError::InputAlreadyConnected { .. });
    }

    #[test]
    fn remove_node_cleans_connections() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        let b = g.add_node(
            "b",
            vec![scalar_in("in"), scalar_out("out")],
            vec![],
        );
        let c = g.add_node("c", vec![scalar_in("in")], vec![]);

        g.connect(
            PortRef { node: a, port: "out".into() },
            PortRef { node: b, port: "in".into() },
        )
        .unwrap();
        g.connect(
            PortRef { node: b, port: "out".into() },
            PortRef { node: c, port: "in".into() },
        )
        .unwrap();

        g.remove_node(b).unwrap();
        assert!(g.connections.is_empty());
    }

    #[test]
    fn serde_round_trip() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("source", vec![scalar_out("out")], vec![]);
        let b = g.add_node("sink", vec![scalar_in("in")], vec![]);
        g.connect(
            PortRef { node: a, port: "out".into() },
            PortRef { node: b, port: "in".into() },
        )
        .unwrap();

        let json = serde_json::to_string(&g).unwrap();
        let g2: Graph<TestWireKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(g2.nodes.len(), 2);
        assert_eq!(g2.connections.len(), 1);
    }
}
