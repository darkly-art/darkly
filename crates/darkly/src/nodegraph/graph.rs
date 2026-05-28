use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::registration::NodeRegistration;
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

/// Display unit for numeric ports.
///
/// Defines how a port's internal value is converted for display in the UI.
/// The conversion methods use `f32` math — any numeric wire type (Scalar,
/// Int) can round-trip through them.  Non-numeric types (Bool, Color)
/// ignore this field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitType {
    /// Identity — display and internal are both raw values (shown as `0.50`).
    #[default]
    Normalized,
    /// Display as percentage: `display = value × 100`, suffix `%`.
    Percent,
    /// Wire unit is radians; display in degrees. `display = value × 180/π`, suffix `°`.
    Degrees,
    /// Identity with no suffix — useful for dimensionless multipliers.
    Raw,
    /// Identity with `px` suffix — value is in canvas pixels.
    Pixels,
}

impl UnitType {
    /// Convert from port-space to display-space.
    pub fn to_display(self, value: f32) -> f32 {
        match self {
            Self::Normalized | Self::Raw | Self::Pixels => value,
            Self::Percent => value * 100.0,
            Self::Degrees => value * (180.0 / std::f32::consts::PI),
        }
    }

    /// Convert from display-space back to port-space.
    pub fn from_display(self, display: f32) -> f32 {
        match self {
            Self::Normalized | Self::Raw | Self::Pixels => display,
            Self::Percent => display / 100.0,
            Self::Degrees => display * (std::f32::consts::PI / 180.0),
        }
    }

    /// Suffix string for display formatting.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Normalized => "",
            Self::Percent => "%",
            Self::Degrees => "°",
            Self::Raw => "",
            Self::Pixels => "px",
        }
    }
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
    /// Quantization step. `0.0` (the default) means continuous; any positive
    /// value snaps the slider, scrub, and typed-value commits to multiples of
    /// `step` from `min`. Used when the wire takes a value but only certain
    /// quantized values produce well-defined behavior — e.g. the circle
    /// node's `frequency`, where only integer values yield a seam-free
    /// closed shape. Frontend honors the snap; the engine should still
    /// defend by quantizing inputs in the node evaluator (a wired-in float
    /// from a curve or pen-pressure modulator bypasses the slider).
    #[serde(default)]
    pub step: f32,
    /// Human-readable description shown as a tooltip in the node editor.
    #[serde(default)]
    pub description: String,
    /// Display unit for numeric ports (controls UI conversion and suffix).
    #[serde(default)]
    pub unit_type: UnitType,
    /// Font Awesome icon class (e.g. `"fa-solid fa-circle"`), or empty.
    #[serde(default)]
    pub icon: String,
    /// User-facing display label.  Falls back to `name` if empty.
    #[serde(default)]
    pub label: String,
    /// Whether this port is exposed in the brush properties panel.
    #[serde(default)]
    pub exposed: bool,
    /// Value substituted for this port during preview rendering. When
    /// set, the preview pipeline calls `apply_preview_overrides`, which
    /// drops any incoming wire on this port and replaces `default`
    /// with this constant. The user's actual port value (defaults,
    /// scrubs, wired modulators) is excluded from the preview entirely.
    ///
    /// This is how a node opts its port out of preview rendering — the
    /// pipeline never inspects the port itself, never knows what it
    /// means, and never branches on its value. Use it for ports whose
    /// value is irrelevant to a brush's *identity* and would distort
    /// the preview if surfaced (canonical example: `stamp.size`, which
    /// is a working scaling factor, not part of how a brush looks).
    #[serde(default)]
    pub preview_value: Option<f32>,
    /// Conditional visibility: the port is only shown in the UI when the
    /// value of the named param is one of the listed integer values. The
    /// param is referenced by its registration name (e.g. `"algorithm"`)
    /// and is expected to be an `Int`/`Enum` param — those are the only
    /// types where dispatch on a discrete value makes sense.
    ///
    /// When `None` (the default), the port is always visible. When set,
    /// the frontend hides the port row whenever the named param's current
    /// value is outside the allowed list. This is purely a UI affordance —
    /// the engine still accepts and reads the port's value normally; it
    /// just stops showing the user a control they wouldn't act on.
    /// Used by the Circle node to hide algorithm-specific knobs (Perlin's
    /// `seed`, Superformula's `n1`/`n2`/`n3`) under the wrong algorithm.
    #[serde(default)]
    pub visible_when: Option<(String, Vec<i32>)>,
    /// Wire-side natural value range. When a connection's source and dest
    /// ports both declare this, the runner remaps the scalar value at
    /// slot-read time from source range to dest range (affine transform).
    /// When either side is `None`, the value passes through raw.
    ///
    /// Distinct from `min`/`max`, which are slider/UI hints — `with_range`
    /// stays "UI hint only, not enforced", and `with_natural_range` is the
    /// separate, explicit opt-in for wire-boundary range mapping. Most
    /// ports declare both with the same numbers; the two diverge for
    /// over-drag sliders like `stamp.size`, where the slider range is
    /// a hint but the wire-side semantics are passthrough.
    #[serde(default)]
    pub natural_range: Option<(f32, f32)>,
    /// Mark this exposed port as part of the brush's *identity* so its
    /// user-set value persists into the dab thumbnail render.
    ///
    /// By default `crate::brush::reset_exposed_scrubs` resets every
    /// exposed input back to its registration default before rendering
    /// the dab thumbnail — the icon represents brush shape/texture, not
    /// the user's working size/opacity/flow knobs. That policy is wrong
    /// for orientation knobs (rotation, phase): a calligraphy nib at
    /// 45° *is* a different-looking brush, and the icon should reflect
    /// that.
    ///
    /// When this flag is set: (1) the reset skips this port, and (2)
    /// scrubbing this port bumps the topology version so the dab
    /// thumbnail re-renders, not just the editor preview.
    #[serde(default)]
    pub persist_in_thumbnail: bool,
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
            description: String::new(),
            unit_type: UnitType::default(),
            icon: String::new(),
            label: String::new(),
            exposed: false,
            preview_value: None,
            visible_when: None,
            step: 0.0,
            natural_range: None,
            persist_in_thumbnail: false,
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
            description: String::new(),
            unit_type: UnitType::default(),
            icon: String::new(),
            label: String::new(),
            exposed: false,
            preview_value: None,
            visible_when: None,
            step: 0.0,
            natural_range: None,
            persist_in_thumbnail: false,
        }
    }

    /// Declare the slider/preset range and default value for this port.
    ///
    /// `(min, max)` is a **UI hint** — bounds for slider widgets and preset
    /// editors.  It is **not enforced at evaluation**: `EvalContext::input_f32`
    /// returns whatever value flowed through the wire (including out-of-range
    /// values from upstream sensors, math nodes, or hand-edited graph data).
    /// Consumers that require a hard bound must clamp explicitly inside
    /// their own `evaluate_gpu` (see e.g. `liquify::evaluate_gpu`'s
    /// `.clamp(0.0, 4.0)`).  A blanket "enforce all declared ranges" would
    /// constrain ports that intentionally accept slider over-drag (notably
    /// `stamp.size`, whose 100% mark is at `1.0` but whose slider extends
    /// further to support dramatically over-sized stamps).
    ///
    /// Separate from [`PortDef::with_natural_range`], which declares the
    /// **wire-side** value semantics used for cross-range remap when two
    /// connected ports speak different ranges. Most ports declare both
    /// with the same numbers; the two diverge for over-drag sliders.
    pub fn with_range(mut self, min: f32, max: f32, default: f32) -> Self {
        self.min = min;
        self.max = max;
        self.default = default;
        self
    }

    /// Declare this port's wire-side natural value range. When a connection's
    /// source and dest ports **both** declare a natural range, the runner
    /// remaps the scalar value at slot-read time (affine transform from
    /// source range to dest range). When either side is `None`, the wire
    /// passes the value through raw — preserving math-node passthrough and
    /// over-drag-slider passthrough (e.g. `stamp.size`).
    ///
    /// Independent of [`PortDef::with_range`], which is a UI/slider hint
    /// only. A port can have a slider range without a natural range (the
    /// over-drag case) or a natural range without a slider (most outputs).
    pub fn with_natural_range(mut self, min: f32, max: f32) -> Self {
        self.natural_range = Some((min, max));
        self
    }

    /// Quantize the port's slider to multiples of `step` from `min`. Pass
    /// `1.0` for an integer-valued port. See [`PortDef::step`] for the full
    /// contract — the engine still needs to defend against non-snapped
    /// values arriving via wires.
    pub fn with_step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    pub fn with_unit(mut self, unit_type: UnitType) -> Self {
        self.unit_type = unit_type;
        self
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Mark this port as exposed in the brush properties panel by default.
    pub fn exposed(mut self) -> Self {
        self.exposed = true;
        self
    }

    /// Opt this port out of preview rendering by spoofing it to a
    /// fixed value. See [`PortDef::preview_value`] for the contract.
    /// Use when the port's user-facing value is a working parameter
    /// (size, position, time) rather than part of the brush's identity.
    pub fn with_preview_value(mut self, value: f32) -> Self {
        self.preview_value = Some(value);
        self
    }

    /// Mark this exposed port as part of the brush's identity — its
    /// user-set value persists into the dab thumbnail, and scrubs of
    /// it rebake the thumbnail. See [`PortDef::persist_in_thumbnail`]
    /// for the contract. Use for orientation knobs (rotation, phase)
    /// that visibly change the dab; don't use for magnitude knobs
    /// (size, opacity, flow) where the icon should stay normalized.
    pub fn persist_in_thumbnail(mut self) -> Self {
        self.persist_in_thumbnail = true;
        self
    }

    /// Show this port in the UI only when the named param's current
    /// integer value is one of `allowed_values`. See [`PortDef::visible_when`]
    /// for the contract. The frontend filters; the engine ignores this
    /// field entirely.
    pub fn with_visible_when(
        mut self,
        param_name: impl Into<String>,
        allowed_values: impl IntoIterator<Item = i32>,
    ) -> Self {
        self.visible_when = Some((param_name.into(), allowed_values.into_iter().collect()));
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

/// Result of [`Graph::find_terminal`]. A graph has exactly one terminal
/// node by construction today; the API surfaces both violations of that
/// invariant so a regression that compiles two terminals (or none) into
/// a brush surfaces loudly rather than silently picking one.
#[derive(Debug, Clone, PartialEq)]
pub enum FindTerminalError {
    /// No node in the graph has `is_terminal: true` in its registration.
    NoTerminal,
    /// More than one node has `is_terminal: true`. Carries every
    /// offending id so the caller can report which.
    MultipleTerminals(Vec<NodeId>),
}

impl std::fmt::Display for FindTerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTerminal => write!(f, "graph has no terminal node"),
            Self::MultipleTerminals(ids) => {
                write!(f, "graph has multiple terminal nodes: {ids:?}")
            }
        }
    }
}

impl std::error::Error for FindTerminalError {}

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
        self.connections
            .iter()
            .filter(move |c| c.to.node == node_id)
    }

    /// All connections whose source is a port on `node_id`.
    pub fn outputs_for(&self, node_id: NodeId) -> impl Iterator<Item = &Connection> {
        self.connections
            .iter()
            .filter(move |c| c.from.node == node_id)
    }

    /// Neutralize ports annotated with [`PortDef::preview_value`] so
    /// the graph renders representably as a preview.
    ///
    /// For each port carrying a `preview_value`, this drops any
    /// incoming wire on the port and replaces its `default` with the
    /// annotated constant. The user's runtime value (whatever scrub
    /// or modulator drove that port) is excluded from the preview.
    /// Ports without a `preview_value` are left alone.
    ///
    /// This is the only place the preview pipeline mutates the graph.
    /// Per-node knowledge — "the preview-time value of *my* port" —
    /// lives on the port registration; the pipeline is brush-agnostic.
    pub fn apply_preview_overrides(&mut self) {
        let mut overrides: Vec<(NodeId, String, f32)> = Vec::new();
        for node in self.nodes.values() {
            for port in &node.ports {
                if let Some(value) = port.preview_value {
                    overrides.push((node.id, port.name.clone(), value));
                }
            }
        }
        for (node_id, port_name, value) in overrides {
            // Drop incoming wires so the spoofed default is what the
            // compiler reads.
            self.connections
                .retain(|c| !(c.to.node == node_id && c.to.port == port_name));
            if let Some(node) = self.nodes.get_mut(&node_id) {
                if let Some(port) = node.ports.iter_mut().find(|p| p.name == port_name) {
                    port.default = value;
                }
            }
        }
    }

    /// Update a port's default value on a node instance.
    ///
    /// This changes the value used when the port is disconnected.
    pub fn set_port_default(
        &mut self,
        id: NodeId,
        port_name: &str,
        value: f32,
    ) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(GraphError::NodeNotFound(id))?;
        let port = node
            .ports
            .iter_mut()
            .find(|p| p.name == port_name && p.dir == PortDir::Input)
            .ok_or_else(|| GraphError::PortNotFound {
                node: id,
                port: port_name.to_string(),
            })?;
        port.default = value;
        Ok(())
    }

    /// Toggle whether an input port is exposed in the brush properties panel.
    pub fn set_port_exposed(
        &mut self,
        id: NodeId,
        port_name: &str,
        exposed: bool,
    ) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(GraphError::NodeNotFound(id))?;
        let port = node
            .ports
            .iter_mut()
            .find(|p| p.name == port_name && p.dir == PortDir::Input)
            .ok_or_else(|| GraphError::PortNotFound {
                node: id,
                port: port_name.to_string(),
            })?;
        port.exposed = exposed;
        Ok(())
    }

    /// Update a single parameter value on a node.
    pub fn set_param(
        &mut self,
        id: NodeId,
        index: usize,
        value: ParamValue,
    ) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(GraphError::NodeNotFound(id))?;
        if index >= node.params.len() {
            return Err(GraphError::PortNotFound {
                node: id,
                port: format!("param[{}]", index),
            });
        }
        node.params[index] = value;
        Ok(())
    }

    /// Find the unique node in this graph whose registration declares
    /// `is_terminal: true`. By today's invariant a brush graph contains
    /// exactly one terminal; deviations are reported via
    /// [`FindTerminalError`] rather than silently arbitrated.
    pub fn find_terminal(
        &self,
        registry: &HashMap<String, NodeRegistration<W>>,
    ) -> Result<NodeId, FindTerminalError> {
        let mut terminals: Vec<NodeId> = self
            .nodes
            .iter()
            .filter_map(|(id, node)| {
                registry
                    .get(&node.type_id)
                    .filter(|r| r.is_terminal)
                    .map(|_| *id)
            })
            .collect();
        match terminals.len() {
            0 => Err(FindTerminalError::NoTerminal),
            1 => Ok(terminals.remove(0)),
            _ => {
                terminals.sort_by_key(|id| id.0);
                Err(FindTerminalError::MultipleTerminals(terminals))
            }
        }
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
        let a = g.add_node("a", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_in("in"), scalar_out("out")], vec![]);

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

        let err = g
            .connect(
                PortRef {
                    node: b,
                    port: "out".into(),
                },
                PortRef {
                    node: a,
                    port: "in".into(),
                },
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
                PortRef {
                    node: a,
                    port: "out".into(),
                },
                PortRef {
                    node: b,
                    port: "in".into(),
                },
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

        let err = g
            .connect(
                PortRef {
                    node: b,
                    port: "out".into(),
                },
                PortRef {
                    node: c,
                    port: "in".into(),
                },
            )
            .unwrap_err();

        matches!(err, GraphError::InputAlreadyConnected { .. });
    }

    #[test]
    fn remove_node_cleans_connections() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")], vec![]);
        let b = g.add_node("b", vec![scalar_in("in"), scalar_out("out")], vec![]);
        let c = g.add_node("c", vec![scalar_in("in")], vec![]);

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

        g.remove_node(b).unwrap();
        assert!(g.connections.is_empty());
    }

    #[test]
    fn serde_round_trip() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("source", vec![scalar_out("out")], vec![]);
        let b = g.add_node("sink", vec![scalar_in("in")], vec![]);
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

        let json = serde_json::to_string(&g).unwrap();
        let g2: Graph<TestWireKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(g2.nodes.len(), 2);
        assert_eq!(g2.connections.len(), 1);
    }

    // ── UnitType tests ──────────────────────────────────────────────

    #[test]
    fn unit_type_conversion_round_trip() {
        for unit in [
            UnitType::Normalized,
            UnitType::Percent,
            UnitType::Degrees,
            UnitType::Raw,
        ] {
            for &val in &[0.0, 0.25, 0.5, 0.75, 1.0] {
                let display = unit.to_display(val);
                let back = unit.from_display(display);
                assert!(
                    (back - val).abs() < 1e-6,
                    "{:?}: to_display({}) = {}, from_display({}) = {} (expected {})",
                    unit,
                    val,
                    display,
                    display,
                    back,
                    val,
                );
            }
        }
    }

    #[test]
    fn unit_type_display_values() {
        use std::f32::consts::PI;
        assert!((UnitType::Percent.to_display(0.5) - 50.0).abs() < 1e-6);
        // Degrees: wire unit is radians, display is degrees.
        assert!((UnitType::Degrees.to_display(PI) - 180.0).abs() < 1e-4);
        assert!((UnitType::Degrees.to_display(PI / 2.0) - 90.0).abs() < 1e-4);
        assert!((UnitType::Degrees.to_display(0.0) - 0.0).abs() < 1e-6);
        assert!((UnitType::Degrees.from_display(90.0) - PI / 2.0).abs() < 1e-4);
        assert!((UnitType::Normalized.to_display(0.5) - 0.5).abs() < 1e-6);
        assert!((UnitType::Raw.to_display(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn unit_type_suffix() {
        assert_eq!(UnitType::Percent.suffix(), "%");
        assert_eq!(UnitType::Degrees.suffix(), "°");
        assert_eq!(UnitType::Normalized.suffix(), "");
        assert_eq!(UnitType::Raw.suffix(), "");
    }

    #[test]
    fn unit_type_serde_round_trip() {
        for unit in [
            UnitType::Normalized,
            UnitType::Percent,
            UnitType::Degrees,
            UnitType::Raw,
        ] {
            let json = serde_json::to_string(&unit).unwrap();
            let back: UnitType = serde_json::from_str(&json).unwrap();
            assert_eq!(unit, back);
        }
    }

    #[test]
    fn port_def_natural_range_round_trip() {
        let port = PortDef::input("seed", TestWireKind::Scalar)
            .with_range(0.0, 1024.0, 0.0)
            .with_natural_range(0.0, 1024.0);
        let json = serde_json::to_string(&port).unwrap();
        let back: PortDef<TestWireKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.natural_range, Some((0.0, 1024.0)));

        // Default builder leaves natural_range unset — opt-in only.
        let bare = PortDef::input("x", TestWireKind::Scalar);
        assert_eq!(bare.natural_range, None);
    }

    #[test]
    fn port_def_step_round_trip() {
        let port = PortDef::input("frequency", TestWireKind::Scalar)
            .with_range(1.0, 16.0, 6.0)
            .with_step(1.0);
        let json = serde_json::to_string(&port).unwrap();
        let back: PortDef<TestWireKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.step, 1.0);
    }

    #[test]
    fn port_def_serde_with_new_fields() {
        let port = PortDef::input("opacity", TestWireKind::Scalar)
            .with_range(0.0, 1.0, 1.0)
            .with_unit(UnitType::Percent)
            .with_icon("fa-solid fa-sun")
            .with_label("Opacity")
            .exposed()
            .with_description("Per-dab opacity");

        let json = serde_json::to_string(&port).unwrap();
        let back: PortDef<TestWireKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.unit_type, UnitType::Percent);
        assert_eq!(back.icon, "fa-solid fa-sun");
        assert_eq!(back.label, "Opacity");
        assert!(back.exposed);
        assert_eq!(back.description, "Per-dab opacity");
    }

    // ── set_port_exposed ────────────────────────────────────────────

    #[test]
    fn set_port_exposed_toggles() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![scalar_in("val")], vec![]);

        assert!(!g.nodes[&id].ports[0].exposed);
        g.set_port_exposed(id, "val", true).unwrap();
        assert!(g.nodes[&id].ports[0].exposed);
        g.set_port_exposed(id, "val", false).unwrap();
        assert!(!g.nodes[&id].ports[0].exposed);
    }

    #[test]
    fn set_port_exposed_wrong_port() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![scalar_out("out")], vec![]);
        // Output ports can't be exposed (set_port_exposed looks for Input).
        let err = g.set_port_exposed(id, "out", true).unwrap_err();
        matches!(err, GraphError::PortNotFound { .. });
    }
}
