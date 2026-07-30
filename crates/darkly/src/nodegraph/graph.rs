use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::registration::NodeRegistration;
use super::WireKind;
use crate::brush::input_value::InputValue;

// ── Identifiers ──────────────────────────────────────────────────────

/// Stable node identity inside a graph. Kind-derived: the first node of a
/// kind gets an id equal to its `type_id` (`"noise"`), the Nth gets
/// `"<type_id>_<N>"` (`"noise_2"`, …). See [`Graph::add_node`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        NodeId(s.to_string())
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        NodeId(s)
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(concrete(W = crate::brush::wire::BrushWireType), bound = "W: ts_rs::TS"))]
pub struct PortDef<W: WireKind> {
    pub name: String,
    pub dir: PortDir,
    pub wire_type: W,
    /// Slider min when the port is disconnected (UI metadata only).
    pub min: f32,
    /// Slider max when the port is disconnected (UI metadata only).
    pub max: f32,
    /// The authored value used when this input port is disconnected — the
    /// full typed value (scalar slider value, enum-dropdown index, texture
    /// name, curve points, color). Wired inputs ignore it and take the
    /// upstream expression. For output ports it stays the neutral scalar
    /// default and is unused. Replaces the old scalar-only `default: f32`;
    /// numeric inputs carry [`InputValue::Scalar`].
    #[serde(default)]
    pub value: InputValue,
    /// Enum-dropdown labels, in index order — non-empty only for
    /// [`WireKind`]-`Enum` inputs (shape's `algorithm`, noise/image `space`,
    /// random's `mode`). Empty for every other input kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_options: Vec<String>,
    /// Whether an upstream wire may drive this input per-dab. Computed from
    /// `wire_type.is_wirable()` at construction and carried as data so the
    /// frontend reads it directly rather than re-deriving the rule — the
    /// single source of truth is [`WireKind::is_wirable`]. Every port built
    /// from a registration (`PortDef::input`/`output`, and the clones in
    /// `add_node` / portable import) sets it correctly; serde round-trips it.
    #[serde(default)]
    pub wirable: bool,
    /// Whether a user may *expose* this input as a brush-bar control.
    /// Computed from `wire_type.is_user_exposable()` at construction and
    /// carried as data so the frontend gates its expose affordance directly
    /// off one value rather than re-deriving the type rule — the single
    /// source of truth is [`WireKind::is_user_exposable`]. Orthogonal to
    /// `wirable`: an enum is exposable but not wirable; a wired scalar is
    /// wirable but (while connected) not user-scrubbable. `expose_port`
    /// enforces it, so a control the brush bar can't render can never be
    /// surfaced. Serde round-trips it.
    #[serde(default)]
    pub exposable: bool,
    /// Quantization step. `0.0` (the default) means continuous; any positive
    /// value snaps the slider, scrub, and typed-value commits to multiples of
    /// `step` from `min`. Used when the wire takes a value but only certain
    /// quantized values produce well-defined behavior — e.g. the shape
    /// node's `frequency`, where only integer values yield a seam-free
    /// closed silhouette. Frontend honors the snap; the engine should still
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
    /// Iconify icon name (e.g. `"fa6-solid:circle"`), or empty.
    #[serde(default)]
    pub icon: String,
    /// User-facing display label.  Falls back to `name` if empty.
    #[serde(default)]
    pub label: String,
    /// Whether this port is exposed in the brush properties panel.
    #[serde(default)]
    pub exposed: bool,
    /// Value substituted for this port in every "brush identity"
    /// render: the cursor-following dab overlay, the editor stroke
    /// preview, and the library thumbnail bake. The brush WGSL
    /// compiler clones the graph, drops incoming wires on flagged
    /// ports, and replaces `default` with this constant — so all
    /// previews read as a showcase of the brush regardless of the
    /// user's working scrub. Real strokes still honour the
    /// configured value.
    ///
    /// Use when the port is something the user actively scrubs but
    /// the preview must stay at a canonical value (otherwise the
    /// preview becomes a moving target as the user dials in their
    /// brush). The picker dab tile uses a more aggressive
    /// neutralizer (`reset_exposed_scrubs`) that targets every
    /// exposed scrub regardless of `preview_value`.
    ///
    /// Canonical example: `paint.size` (0.1, so a huge brush's
    /// preview still fits the small cursor mask and the editor
    /// preview doesn't redraw on every size scrub).
    #[serde(default)]
    pub preview_value: Option<f32>,
    /// Declares that scrubbing this port's value does **not** change
    /// what the synthetic-stroke editor preview produces, so the
    /// preview cache and version counter should not bump on its scrub.
    ///
    /// Used by ports whose value the preview *pipeline* (not the
    /// shader) ignores. Canonical example: `pen_input.stabilize` —
    /// the editor preview's stroke engine is hard-wired to use
    /// `PassThrough` as the stabilizer (the path is pre-cooked), so
    /// the live `stabilize` value never reaches it. Marking this
    /// declaratively avoids re-rendering a full stroke every ~100 ms
    /// while the user drags the slider for no visible effect.
    ///
    /// Distinct from [`PortDef::preview_value`]: that one substitutes
    /// values into the *cursor overlay shader*; this one skips a
    /// version bump on the *editor stroke preview*. A port can carry
    /// either, both, or neither.
    #[serde(default)]
    pub preview_irrelevant_scrub: bool,
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
    /// Used by the Shape node to hide algorithm-specific knobs (Perlin's
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
    /// over-drag sliders like `paint.size`, where the slider range is
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
    /// for orientation knobs (rotation): a calligraphy nib at
    /// 45° *is* a different-looking brush, and the icon should reflect
    /// that.
    ///
    /// When this flag is set: (1) the reset skips this port, and (2)
    /// scrubbing this port bumps the topology version so the dab
    /// thumbnail re-renders, not just the editor preview.
    #[serde(default)]
    pub persist_in_thumbnail: bool,
    /// This output port emits a *spatial, per-fragment image* — a coverage
    /// mask or colour field that varies across the dab — so a node carrying it
    /// is worth a preview thumbnail (`circle.mask`, `image.color`,
    /// `noise.color`, `stamp.dab`). Declared per port rather than inferred
    /// from `wire_type`, because wire type can't tell a spatial field from a
    /// per-dab constant: `random.value` and `paint_color.color` share the
    /// `Scalar`/`Vec4` types with the real image outputs but render as flat
    /// blobs. The node-preview builder wires the first port carrying this flag;
    /// the brush-builder's preview gate reads it directly (like `wirable` /
    /// `exposable`). Meaningless on inputs; only set on outputs.
    #[serde(default)]
    pub preview_image: bool,
    /// This input port is *also* a wire source: its resolved value (the
    /// wired value if driven, else the authored default) is available for
    /// other nodes to wire *from*, exactly like an output. Only meaningful
    /// on `dir == Input`; ignored on outputs (which are sources anyway).
    ///
    /// The editor shows the source handle only while the input is not
    /// itself wire-driven — a driven port's value is the driver's, so it
    /// should be tapped there instead. Consumers that resolve "which port a
    /// wire leaves from" must ask [`PortDef::is_source`], never
    /// `dir == Output`, or a settable-source is treated as a second-class
    /// source (skipped by wire-range remap, unreachable by `find_port`).
    #[serde(default)]
    pub source: bool,
}

impl<W: WireKind> PortDef<W> {
    pub fn input(name: impl Into<String>, wire_type: W) -> Self {
        Self {
            name: name.into(),
            dir: PortDir::Input,
            wire_type,
            min: 0.0,
            max: 1.0,
            value: InputValue::Scalar(0.0),
            enum_options: Vec::new(),
            wirable: wire_type.is_wirable(),
            exposable: wire_type.is_user_exposable(),
            description: String::new(),
            unit_type: UnitType::default(),
            icon: String::new(),
            label: String::new(),
            exposed: false,
            preview_value: None,
            preview_irrelevant_scrub: false,
            visible_when: None,
            step: 0.0,
            natural_range: None,
            persist_in_thumbnail: false,
            preview_image: false,
            source: false,
        }
    }

    pub fn output(name: impl Into<String>, wire_type: W) -> Self {
        Self {
            name: name.into(),
            dir: PortDir::Output,
            wire_type,
            min: 0.0,
            max: 1.0,
            value: InputValue::Scalar(0.0),
            enum_options: Vec::new(),
            wirable: wire_type.is_wirable(),
            exposable: wire_type.is_user_exposable(),
            description: String::new(),
            unit_type: UnitType::default(),
            icon: String::new(),
            label: String::new(),
            exposed: false,
            preview_value: None,
            preview_irrelevant_scrub: false,
            visible_when: None,
            step: 0.0,
            natural_range: None,
            persist_in_thumbnail: false,
            preview_image: false,
            source: false,
        }
    }

    /// `true` if a wire may leave this port — every output, plus a
    /// settable-source input (see [`PortDef::source`]). The single predicate
    /// every wire-source resolution must use instead of `dir == Output`.
    pub fn is_source(&self) -> bool {
        self.dir == PortDir::Output || (self.dir == PortDir::Input && self.source)
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
        self.value = InputValue::Scalar(default);
        self
    }

    /// Set this input's authored value directly — the general form behind
    /// [`Self::with_range`]. Use for the non-scalar input kinds: an
    /// [`InputValue::Int`] enum index, an [`InputValue::String`] texture
    /// name, [`InputValue::Curve`] points, an [`InputValue::Bool`] flag.
    pub fn with_value(mut self, value: InputValue) -> Self {
        self.value = value;
        self
    }

    /// Declare the dropdown labels for an [`WireKind`]-`Enum` input, in
    /// index order. The stored value is the selected index.
    pub fn with_enum_options<I, S>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.enum_options = options.into_iter().map(Into::into).collect();
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

    /// Mark this input port as a settable-source: its resolved value is also
    /// available as a wire source. See [`PortDef::source`] / [`PortDef::is_source`].
    pub fn source(mut self) -> Self {
        self.source = true;
        self
    }

    /// Declare that this output port emits a spatial per-fragment image worth
    /// previewing. See [`PortDef::preview_image`] for the contract. Set it on
    /// coverage/colour-field outputs (`circle.mask`, `image.color`, …); leave
    /// it off for per-dab constants and sensor/math outputs.
    pub fn preview_image(mut self) -> Self {
        self.preview_image = true;
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

    /// Declare that this port's value is ignored by the synthetic-stroke
    /// editor preview pipeline, so the editor preview's cache need not
    /// rebuild on its scrub. See [`PortDef::preview_irrelevant_scrub`]
    /// for the contract.
    pub fn preview_irrelevant_scrub(mut self) -> Self {
        self.preview_irrelevant_scrub = true;
        self
    }

    /// Mark this exposed port as part of the brush's identity — its
    /// user-set value persists into the dab thumbnail, and scrubs of
    /// it rebake the thumbnail. See [`PortDef::persist_in_thumbnail`]
    /// for the contract. Use for orientation knobs (rotation)
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
    /// Port definitions (copied from registration on creation). This is the
    /// node's single, unified input/output list — the per-instance authored
    /// value of every input lives on its [`PortDef::value`].
    pub ports: Vec<PortDef<W>>,
    /// Free-form author annotation on this node instance. Empty means none.
    /// Inert w.r.t. compilation and render output; carried purely so a brush
    /// author can leave explanatory notes on a node. Serializable graph state
    /// (survives save/load through both the portable YAML and the bundle).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
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
    /// The destination input's wire type is not wirable (an enum, string,
    /// or curve that resolves at compile time). Makes "unwirable" a
    /// structural invariant the connect path enforces, so a hand-edited
    /// graph or paste can't smuggle an illegal wire in.
    InputNotWirable {
        node: NodeId,
        port: String,
    },
    /// The input can't be surfaced as a brush-bar control because its wire
    /// type has no editing widget (`Curve`, `String`, …). Makes "exposable"
    /// a structural invariant the expose path enforces, mirroring how
    /// `InputNotWirable` guards `connect` — a control the bar can't render
    /// can never be exposed in the first place.
    PortNotExposable {
        node: NodeId,
        port: String,
    },
    /// `exposed_ports` lookup by key (`"<node_id>.<port>"`) failed.
    ExposedPortNotFound {
        key: String,
    },
    /// Icon string contained a character outside the Iconify-name
    /// shape (`[a-zA-Z0-9-: ]`). Rejected so the value stays a safe,
    /// inert token (it is passed to `<Icon name={...}>`, never `{@html}`).
    InvalidIcon {
        icon: String,
    },
}

/// Accept only the byte shape Iconify names use (`prefix:name`):
/// letters, digits, hyphens, and the `:` separator (spaces tolerated for
/// legacy values). Keeping the value within this alphabet means the stored
/// string is an inert token — the frontend hands it to `<Icon name={...}>`,
/// which resolves it against the offline bundle and renders nothing for an
/// unknown name, so a hostile value can never become markup.
fn is_safe_icon_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b':' || b == b' '
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
            Self::InputNotWirable { node, port } => {
                write!(f, "input '{}' on {:?} is not wirable", port, node)
            }
            Self::PortNotExposable { node, port } => {
                write!(f, "input '{}' on {:?} is not user-exposable", port, node)
            }
            Self::ExposedPortNotFound { key } => {
                write!(f, "exposed-port entry '{}' not found", key)
            }
            Self::InvalidIcon { icon } => {
                write!(
                    f,
                    "icon '{}' contains characters outside [a-zA-Z0-9- ]",
                    icon
                )
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

// ── Exposed port metadata ────────────────────────────────────────────

/// Per-placement metadata for an entry in a graph's `exposed_ports` map.
/// All fields are optional: empty strings fall back to the registration's
/// `PortDef::label` / `PortDef::description` / `PortDef::icon` when the
/// brush bar renders the entry.
///
/// Lives in `Graph::exposed_ports` rather than on `PortDef` per instance
/// because the brush bar is a single user-facing surface — centralizing
/// "what the user sees" in one ordered dict makes display order natural
/// (map iteration order is the brush-bar order) and gives the
/// brush-author editor one canonical place to read and write.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposedPortMeta {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
}

/// Format the canonical key for an exposed-port entry. Keys are
/// `"<node_id>.<port_name>"` strings so the dict round-trips through
/// JSON/YAML without needing tuple-key encoding.
pub fn exposed_port_key(node: &NodeId, port: &str) -> String {
    format!("{}.{}", node.0, port)
}

// ── Graph ────────────────────────────────────────────────────────────

/// A directed acyclic graph of nodes connected by typed wires.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Graph<W: WireKind> {
    nodes: HashMap<NodeId, NodeInstance<W>>,
    pub connections: Vec<Connection>,
    /// Ordered set of exposed-port entries. Insertion order is the
    /// brush-bar display order — `IndexMap` preserves it through every
    /// mutation and JSON/YAML round-trip. Keys come from
    /// [`exposed_port_key`].
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub exposed_ports: IndexMap<String, ExposedPortMeta>,
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
            exposed_ports: IndexMap::new(),
        }
    }

    /// Read-only access to the node map. The graph owns id assignment
    /// (via [`add_node`](Self::add_node)) and the invariant that every
    /// connection references existing nodes, so external code must go
    /// through the graph's methods to mutate the set.
    pub fn nodes(&self) -> &HashMap<NodeId, NodeInstance<W>> {
        &self.nodes
    }

    /// Add a node and return its assigned id. Any input port whose
    /// registration `PortDef` declares `.exposed()` is auto-appended to
    /// `exposed_ports` with empty meta — preserves the "size etc. are
    /// exposed by default" affordance.
    pub fn add_node(&mut self, type_id: impl Into<String>, ports: Vec<PortDef<W>>) -> NodeId {
        let type_id = type_id.into();
        let id = self.unique_id_for(&type_id);
        // Walk before move: every input port flagged exposed gets a
        // default brush-bar entry.
        for port in ports.iter() {
            if port.dir == PortDir::Input && port.exposed && port.exposable {
                let key = exposed_port_key(&id, &port.name);
                self.exposed_ports.insert(key, ExposedPortMeta::default());
            }
        }
        self.nodes.insert(
            id.clone(),
            NodeInstance {
                id: id.clone(),
                type_id,
                ports,
                comment: String::new(),
            },
        );
        id
    }

    /// Derive a fresh, unique id for a node of kind `type_id`. The first
    /// node of a kind gets id == its `type_id` (`"noise"`); the Nth gets
    /// `"<type_id>_<N>"` (`"noise_2"`, `"noise_3"`, …). Probes upward until
    /// the candidate is free, so it stays correct after arbitrary removals
    /// (a freed `"noise_2"` is reused before minting `"noise_3"`).
    ///
    /// Determinism: the node map is the source of truth for "which ids are
    /// taken." When a file is imported, same-kind disambiguation follows the
    /// order `add_node` is called, which is the `BTreeMap`-key order of the
    /// source file (lexicographic) — the lexicographically-first same-kind
    /// key normalizes to the bare `type_id`.
    fn unique_id_for(&self, type_id: &str) -> NodeId {
        let base = NodeId(type_id.to_string());
        if !self.nodes.contains_key(&base) {
            return base;
        }
        let mut n = 2;
        loop {
            let cand = NodeId(format!("{type_id}_{n}"));
            if !self.nodes.contains_key(&cand) {
                return cand;
            }
            n += 1;
        }
    }

    /// Remove a node, all its connections, and every brush-bar entry
    /// referencing one of its ports.
    pub fn remove_node(&mut self, id: &NodeId) -> Result<(), GraphError> {
        if self.nodes.remove(id).is_none() {
            return Err(GraphError::NodeNotFound(id.clone()));
        }
        self.connections
            .retain(|c| &c.from.node != id && &c.to.node != id);
        let prefix = format!("{}.", id.0);
        self.exposed_ports
            .retain(|key, _| !key.starts_with(&prefix));
        Ok(())
    }

    /// Add a brush-bar entry for an input port, no-op if already present.
    /// The entry starts with empty meta (registration values are used as
    /// fallback at render time).
    pub fn expose_port(&mut self, id: &NodeId, port_name: &str) -> Result<(), GraphError> {
        // Validate that the port exists and is an input.
        let node = self
            .nodes
            .get(id)
            .ok_or_else(|| GraphError::NodeNotFound(id.clone()))?;
        let port = node
            .ports
            .iter()
            .find(|p| p.name == port_name && p.dir == PortDir::Input)
            .ok_or_else(|| GraphError::PortNotFound {
                node: id.clone(),
                port: port_name.to_string(),
            })?;
        // Only controls the brush bar can render may be exposed — enforce it
        // here so an un-renderable entry can never reach the read builder.
        if !port.exposable {
            return Err(GraphError::PortNotExposable {
                node: id.clone(),
                port: port_name.to_string(),
            });
        }
        let key = exposed_port_key(id, port_name);
        self.exposed_ports.entry(key).or_default();
        Ok(())
    }

    /// Drop a brush-bar entry by `(node, port)`. Idempotent — missing
    /// entries are not an error.
    pub fn unexpose_port(&mut self, id: &NodeId, port_name: &str) {
        let key = exposed_port_key(id, port_name);
        self.exposed_ports.shift_remove(&key);
    }

    /// Returns true when the named input port has a live brush-bar entry.
    pub fn is_port_exposed(&self, id: &NodeId, port_name: &str) -> bool {
        self.exposed_ports
            .contains_key(&exposed_port_key(id, port_name))
    }

    /// Overwrite all three meta fields on a brush-bar entry in one call.
    /// The icon field is restricted to FontAwesome-friendly characters
    /// (`[a-zA-Z0-9- ]*`) — keeps the value safe to bind directly into
    /// an HTML `class=` attribute on the frontend without further
    /// sanitization. Out-of-shape icon strings are rejected loudly so
    /// the caller learns about the constraint rather than seeing the
    /// icon silently dropped.
    pub fn set_exposed_port_meta(
        &mut self,
        key: &str,
        label: String,
        description: String,
        icon: String,
    ) -> Result<(), GraphError> {
        if !icon.bytes().all(is_safe_icon_byte) {
            return Err(GraphError::InvalidIcon { icon });
        }
        let entry =
            self.exposed_ports
                .get_mut(key)
                .ok_or_else(|| GraphError::ExposedPortNotFound {
                    key: key.to_string(),
                })?;
        entry.label = label;
        entry.description = description;
        entry.icon = icon;
        Ok(())
    }

    /// Move an exposed-port entry to position `new_index`. The map's
    /// iteration order is the brush-bar display order, so this is how
    /// drag-reorder is realised. `new_index` is clamped to the map's
    /// length.
    pub fn reorder_exposed_port(&mut self, key: &str, new_index: usize) -> Result<(), GraphError> {
        let from = self.exposed_ports.get_index_of(key).ok_or_else(|| {
            GraphError::ExposedPortNotFound {
                key: key.to_string(),
            }
        })?;
        let target = new_index.min(self.exposed_ports.len().saturating_sub(1));
        self.exposed_ports.move_index(from, target);
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

        // Wirability check: a compile-time input (enum, string, curve) can
        // never accept a per-dab wire. Type-owned — asks the wire type, not
        // a consumer-side classifier.
        if !to_def.is_wirable() {
            return Err(GraphError::InputNotWirable {
                node: to.node.clone(),
                port: to.port.clone(),
            });
        }

        // Input-already-connected check.
        if self.connections.iter().any(|c| c.to == to) {
            return Err(GraphError::InputAlreadyConnected {
                node: to.node.clone(),
                port: to.port.clone(),
            });
        }

        // Cycle check: would adding from→to create a cycle?
        // A cycle exists iff `from.node` is reachable from `to.node`
        // through existing connections (i.e., to is upstream of from).
        if self.is_reachable(&to.node, &from.node) {
            return Err(GraphError::CycleDetected);
        }

        // Driving a settable-source input retires any wires that were tapping
        // it as a source: once driven, the port's value is the driver's, and
        // the editor hides its source handle (see `PortDef::source`), so the
        // graph must not keep carrying the old knob value downstream. A plain
        // input has no outgoing wires, so this is a no-op for it.
        self.connections
            .retain(|c| !(c.from.node == to.node && c.from.port == to.port));

        self.connections.push(Connection { from, to });
        Ok(())
    }

    /// Disconnect a specific wire.
    pub fn disconnect(&mut self, from: &PortRef, to: &PortRef) {
        self.connections.retain(|c| &c.from != from || &c.to != to);
    }

    /// All connections whose destination is a port on `node_id`.
    pub fn inputs_for<'a>(&'a self, node_id: &'a NodeId) -> impl Iterator<Item = &'a Connection> {
        self.connections
            .iter()
            .filter(move |c| &c.to.node == node_id)
    }

    /// All connections whose source is a port on `node_id`.
    pub fn outputs_for<'a>(&'a self, node_id: &'a NodeId) -> impl Iterator<Item = &'a Connection> {
        self.connections
            .iter()
            .filter(move |c| &c.from.node == node_id)
    }

    /// Neutralize ports annotated with [`PortDef::preview_value`] so
    /// the graph compiles to a cursor-overlay-friendly preview shader.
    ///
    /// For each port carrying a `preview_value`, this drops any incoming
    /// wire on the port and replaces its `default` with the annotated
    /// constant. Ports without a `preview_value` are left alone.
    ///
    /// Called by every renderer that wants brush-identity output rather
    /// than the user's momentary scrub state:
    /// - the WGSL compiler, on a clone, before emitting
    ///   `CompiledBrush::cursor_preview_wgsl` (the cursor halo);
    /// - the brush-editor stroke preview;
    /// - the library thumbnail bake (`brush_save`, `brush_thumbnail`).
    ///
    /// The picker dab tile uses a different, more aggressive neutralizer
    /// (`reset_exposed_scrubs`) that resets every exposed scrub to its
    /// registration default. Both kinds of preview want the same end:
    /// scrubbing any `preview_value`-tagged port shouldn't redraw the
    /// preview, because the rendered output is identical by construction.
    pub(crate) fn apply_preview_overrides(&mut self) {
        let mut overrides: Vec<(NodeId, String, f32)> = Vec::new();
        for node in self.nodes.values() {
            for port in &node.ports {
                if let Some(value) = port.preview_value {
                    overrides.push((node.id.clone(), port.name.clone(), value));
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
                    port.value = InputValue::Scalar(value);
                }
            }
        }
    }

    /// Update an input port's authored value on a node instance — the value
    /// used when the port is disconnected. The general form; see
    /// [`Self::set_port_default`] for the scalar convenience wrapper the hot
    /// slider path uses.
    pub fn set_port_value(
        &mut self,
        id: &NodeId,
        port_name: &str,
        value: InputValue,
    ) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::NodeNotFound(id.clone()))?;
        let port = node
            .ports
            .iter_mut()
            .find(|p| p.name == port_name && p.dir == PortDir::Input)
            .ok_or_else(|| GraphError::PortNotFound {
                node: id.clone(),
                port: port_name.to_string(),
            })?;
        port.value = value;
        Ok(())
    }

    /// Set (or clear, with an empty string) a node's author comment.
    pub fn set_node_comment(&mut self, id: &NodeId, comment: String) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::NodeNotFound(id.clone()))?;
        node.comment = comment;
        Ok(())
    }

    /// Scalar convenience wrapper over [`Self::set_port_value`] — the hot
    /// path for slider scrubs and numeric port defaults. Wraps the `f32` in
    /// [`InputValue::Scalar`].
    pub fn set_port_default(
        &mut self,
        id: &NodeId,
        port_name: &str,
        value: f32,
    ) -> Result<(), GraphError> {
        self.set_port_value(id, port_name, InputValue::Scalar(value))
    }

    // Note: brush-bar exposure / label / description / icon overrides
    // live in `Graph::exposed_ports` now. Use `expose_port`,
    // `unexpose_port`, `set_exposed_port_meta`, and `reorder_exposed_port`.

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
                    .map(|_| id.clone())
            })
            .collect();
        match terminals.len() {
            0 => Err(FindTerminalError::NoTerminal),
            1 => Ok(terminals.remove(0)),
            _ => {
                terminals.sort_by(|a, b| a.0.cmp(&b.0));
                Err(FindTerminalError::MultipleTerminals(terminals))
            }
        }
    }

    // ── helpers ──────────────────────────────────────────────────────

    /// Find the wire type of a port, returning an error if the node or
    /// port doesn't exist or can't play the requested role. `expected_dir`
    /// names the *role* the endpoint plays on a wire: `Output` = the source
    /// end (resolved by [`PortDef::is_source`], so settable-source inputs
    /// qualify), `Input` = the sink end.
    fn find_port(&self, pr: &PortRef, expected_dir: PortDir) -> Result<W, GraphError> {
        let node = self
            .nodes
            .get(&pr.node)
            .ok_or_else(|| GraphError::NodeNotFound(pr.node.clone()))?;
        let matches = |p: &&PortDef<W>| {
            p.name == pr.port
                && match expected_dir {
                    PortDir::Output => p.is_source(),
                    PortDir::Input => p.dir == PortDir::Input,
                }
        };
        let def = node
            .ports
            .iter()
            .find(matches)
            .ok_or_else(|| GraphError::PortNotFound {
                node: pr.node.clone(),
                port: pr.port.clone(),
            })?;
        Ok(def.wire_type)
    }

    /// DFS reachability: can we get from `start` to `target` following
    /// existing connection edges (from.node → to.node)?
    fn is_reachable(&self, start: &NodeId, target: &NodeId) -> bool {
        let mut visited = HashSet::new();
        let mut stack = vec![start.clone()];
        while let Some(current) = stack.pop() {
            if &current == target {
                return true;
            }
            if !visited.insert(current.clone()) {
                continue;
            }
            for conn in &self.connections {
                if conn.from.node == current {
                    stack.push(conn.to.node.clone());
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

    fn scalar_source(name: &str) -> PortDef<TestWireKind> {
        PortDef::input(name, TestWireKind::Scalar).source()
    }

    /// The first node of a kind gets an id equal to its `type_id`.
    #[test]
    fn add_node_first_of_kind_uses_type_id() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("noise", vec![scalar_out("out")]);
        assert_eq!(id, NodeId("noise".into()));
    }

    /// Second/third nodes of the same kind get `_2`, `_3` suffixes.
    #[test]
    fn add_node_second_of_kind_gets_suffix() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("noise", vec![scalar_out("out")]);
        let b = g.add_node("noise", vec![scalar_out("out")]);
        let c = g.add_node("noise", vec![scalar_out("out")]);
        assert_eq!(a, NodeId("noise".into()));
        assert_eq!(b, NodeId("noise_2".into()));
        assert_eq!(c, NodeId("noise_3".into()));
    }

    /// Removing a node frees its id; the next add of that kind probes into
    /// the gap rather than minting a fresh suffix.
    #[test]
    fn add_node_reuses_freed_id() {
        let mut g = Graph::<TestWireKind>::new();
        let _a = g.add_node("noise", vec![scalar_out("out")]);
        let b = g.add_node("noise", vec![scalar_out("out")]);
        assert_eq!(b, NodeId("noise_2".into()));
        g.remove_node(&b).unwrap();
        let c = g.add_node("noise", vec![scalar_out("out")]);
        assert_eq!(c, NodeId("noise_2".into()));
    }

    /// A settable-source input resolves as a wire *source* (its `is_source()`
    /// is honoured by `connect`'s from-side), while still being a settable
    /// input on the same node — the two never interfere.
    #[test]
    fn settable_source_is_both_wire_source_and_settable_input() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("knob", vec![scalar_source("val")]);
        let b = g.add_node("sink", vec![scalar_in("in")]);

        // Wire *from* the settable-source input into another node's input.
        g.connect(
            PortRef {
                node: a.clone(),
                port: "val".into(),
            },
            PortRef {
                node: b,
                port: "in".into(),
            },
        )
        .expect("settable-source resolves as a wire source");
        assert_eq!(g.connections.len(), 1);

        // Setting its default targets the input side and doesn't disturb the
        // wire leaving it.
        g.set_port_default(&a, "val", 0.5).unwrap();
        assert_eq!(g.connections.len(), 1);
    }

    /// Driving a settable-source input (wiring *into* it) retires any wires
    /// that were tapping it as a source — its value is now the driver's.
    #[test]
    fn driving_a_settable_source_drops_its_outgoing_wires() {
        let mut g = Graph::<TestWireKind>::new();
        let knob = g.add_node("knob", vec![scalar_source("val")]);
        let sink = g.add_node("sink", vec![scalar_in("in")]);
        let driver = g.add_node("driver", vec![scalar_out("out")]);

        g.connect(
            PortRef {
                node: knob.clone(),
                port: "val".into(),
            },
            PortRef {
                node: sink,
                port: "in".into(),
            },
        )
        .unwrap();
        assert_eq!(g.connections.len(), 1);

        // Now drive the knob from `driver`. The knob→sink source wire must go.
        g.connect(
            PortRef {
                node: driver,
                port: "out".into(),
            },
            PortRef {
                node: knob.clone(),
                port: "val".into(),
            },
        )
        .unwrap();
        assert_eq!(
            g.connections.len(),
            1,
            "outgoing source wire should be dropped"
        );
        assert!(
            g.connections
                .iter()
                .all(|c| c.to.node == knob && c.to.port == "val"),
            "only the driver→knob wire should remain",
        );
    }

    #[test]
    fn add_connect_disconnect_remove() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("source", vec![scalar_out("out")]);
        let b = g.add_node("sink", vec![scalar_in("in")]);

        let from = PortRef {
            node: a.clone(),
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

        g.remove_node(&a).unwrap();
        assert!(!g.nodes.contains_key(&a));
    }

    #[test]
    fn cycle_detection() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_in("in"), scalar_out("out")]);
        let b = g.add_node("b", vec![scalar_in("in"), scalar_out("out")]);

        g.connect(
            PortRef {
                node: a.clone(),
                port: "out".into(),
            },
            PortRef {
                node: b.clone(),
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
        let a = g.add_node("a", vec![color_out("out")]);
        let b = g.add_node("b", vec![scalar_in("in")]);

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
    fn connect_rejects_non_wirable_input() {
        // A type-compatible wire into a non-wirable input is refused
        // structurally, so a hand-edited graph or paste can't smuggle in a
        // wire the model forbids.
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![PortDef::output("out", TestWireKind::Data)]);
        let b = g.add_node("b", vec![PortDef::input("in", TestWireKind::Data)]);
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
        assert!(matches!(err, GraphError::InputNotWirable { .. }));
        assert!(g.connections.is_empty());
    }

    #[test]
    fn input_already_connected() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("a", vec![scalar_out("out")]);
        let b = g.add_node("b", vec![scalar_out("out")]);
        let c = g.add_node("c", vec![scalar_in("in")]);

        g.connect(
            PortRef {
                node: a,
                port: "out".into(),
            },
            PortRef {
                node: c.clone(),
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
        let a = g.add_node("a", vec![scalar_out("out")]);
        let b = g.add_node("b", vec![scalar_in("in"), scalar_out("out")]);
        let c = g.add_node("c", vec![scalar_in("in")]);

        g.connect(
            PortRef {
                node: a,
                port: "out".into(),
            },
            PortRef {
                node: b.clone(),
                port: "in".into(),
            },
        )
        .unwrap();
        g.connect(
            PortRef {
                node: b.clone(),
                port: "out".into(),
            },
            PortRef {
                node: c,
                port: "in".into(),
            },
        )
        .unwrap();

        g.remove_node(&b).unwrap();
        assert!(g.connections.is_empty());
    }

    #[test]
    fn serde_round_trip() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("source", vec![scalar_out("out")]);
        let b = g.add_node("sink", vec![scalar_in("in")]);
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

    #[test]
    fn set_node_comment_sets_and_clears() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("source", vec![scalar_out("out")]);
        assert_eq!(g.nodes[&a].comment, "");

        g.set_node_comment(&a, "explanatory wisdom".into()).unwrap();
        assert_eq!(g.nodes[&a].comment, "explanatory wisdom");

        g.set_node_comment(&a, String::new()).unwrap();
        assert_eq!(g.nodes[&a].comment, "");
    }

    #[test]
    fn set_node_comment_unknown_node_errors() {
        let mut g = Graph::<TestWireKind>::new();
        let err = g
            .set_node_comment(&NodeId("ghost".into()), "hi".into())
            .unwrap_err();
        assert_eq!(err, GraphError::NodeNotFound(NodeId("ghost".into())));
    }

    /// A comment is inert but must survive the raw-`Graph` serde that backs
    /// the `.darkly-brush` bundle. Empty comments are elided from the JSON.
    #[test]
    fn comment_survives_serde_and_elides_when_empty() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("source", vec![scalar_out("out")]);
        let b = g.add_node("sink", vec![scalar_in("in")]);
        g.set_node_comment(&a, "keep me".into()).unwrap();

        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(json.matches("\"comment\"").count(), 1);

        let g2: Graph<TestWireKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(g2.nodes[&a].comment, "keep me");
        assert_eq!(g2.nodes[&b].comment, "");
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
            .with_icon("fa6-solid:sun")
            .with_label("Opacity")
            .exposed()
            .with_description("Per-dab opacity");

        let json = serde_json::to_string(&port).unwrap();
        let back: PortDef<TestWireKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.unit_type, UnitType::Percent);
        assert_eq!(back.icon, "fa6-solid:sun");
        assert_eq!(back.label, "Opacity");
        assert!(back.exposed);
        assert_eq!(back.description, "Per-dab opacity");
    }

    // ── exposed_ports ──────────────────────────────────────────────

    #[test]
    fn expose_then_unexpose_round_trips() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![scalar_in("val")]);

        assert!(!g.is_port_exposed(&id, "val"));
        g.expose_port(&id, "val").unwrap();
        assert!(g.is_port_exposed(&id, "val"));
        // Idempotent — double-expose doesn't add a duplicate.
        g.expose_port(&id, "val").unwrap();
        assert_eq!(g.exposed_ports.len(), 1);
        g.unexpose_port(&id, "val");
        assert!(!g.is_port_exposed(&id, "val"));
    }

    #[test]
    fn expose_port_rejects_output_port() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![scalar_out("out")]);
        let err = g.expose_port(&id, "out").unwrap_err();
        assert!(matches!(err, GraphError::PortNotFound { .. }));
    }

    #[test]
    fn expose_port_rejects_non_exposable_input() {
        // `Data` is a compile-time shape with no editing widget
        // (`is_user_exposable() == false`). Exposing it is rejected at the
        // model boundary, so a control the brush bar can't render can never
        // land in `exposed_ports`.
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![PortDef::input("d", TestWireKind::Data)]);
        let err = g.expose_port(&id, "d").unwrap_err();
        assert!(matches!(err, GraphError::PortNotExposable { .. }));
        assert!(!g.is_port_exposed(&id, "d"));
    }

    #[test]
    fn add_node_does_not_auto_expose_non_exposable_port() {
        // Even a registration that flags a non-exposable input `.exposed()`
        // must not seed a brush-bar entry — the exposability invariant holds
        // by construction, not just at the interactive expose call.
        let mut g = Graph::<TestWireKind>::new();
        let mut d = PortDef::input("d", TestWireKind::Data);
        d.exposed = true;
        let id = g.add_node("node", vec![d]);
        assert!(!g.is_port_exposed(&id, "d"));
        assert!(g.exposed_ports.is_empty());
    }

    #[test]
    fn add_node_seeds_exposed_from_registration_flag() {
        let mut g = Graph::<TestWireKind>::new();
        // Two inputs: only the second is flagged `.exposed()` at the
        // registration level. add_node should auto-append it to
        // exposed_ports with empty meta.
        let mut a = scalar_in("a");
        let mut b = scalar_in("b");
        a.exposed = false;
        b.exposed = true;
        let id = g.add_node("node", vec![a, b]);
        assert!(!g.is_port_exposed(&id, "a"));
        assert!(g.is_port_exposed(&id, "b"));
        // Empty meta — falls back to registration at render time.
        let key = exposed_port_key(&id, "b");
        assert_eq!(g.exposed_ports[&key], ExposedPortMeta::default());
    }

    #[test]
    fn remove_node_drops_exposed_entries() {
        let mut g = Graph::<TestWireKind>::new();
        let a = g.add_node("node", vec![scalar_in("x"), scalar_in("y")]);
        let b = g.add_node("node", vec![scalar_in("x")]);
        g.expose_port(&a, "x").unwrap();
        g.expose_port(&a, "y").unwrap();
        g.expose_port(&b, "x").unwrap();
        assert_eq!(g.exposed_ports.len(), 3);

        g.remove_node(&a).unwrap();
        // Only b.x survives.
        assert_eq!(g.exposed_ports.len(), 1);
        assert!(g.is_port_exposed(&b, "x"));
    }

    #[test]
    fn reorder_moves_entry_to_target_index() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![scalar_in("a"), scalar_in("b"), scalar_in("c")]);
        g.expose_port(&id, "a").unwrap();
        g.expose_port(&id, "b").unwrap();
        g.expose_port(&id, "c").unwrap();
        let keys: Vec<&str> = g.exposed_ports.keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 3);

        // Move b to index 0.
        let b_key = exposed_port_key(&id, "b");
        g.reorder_exposed_port(&b_key, 0).unwrap();

        let order: Vec<&str> = g.exposed_ports.keys().map(String::as_str).collect();
        let a_key = exposed_port_key(&id, "a");
        let c_key = exposed_port_key(&id, "c");
        assert_eq!(order, vec![b_key.as_str(), a_key.as_str(), c_key.as_str()]);
    }

    #[test]
    fn set_meta_rejects_unsafe_icon() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![scalar_in("val")]);
        g.expose_port(&id, "val").unwrap();
        let key = exposed_port_key(&id, "val");

        // Safe icon class — accepted.
        g.set_exposed_port_meta(&key, "Label".into(), "Desc".into(), "fa6-solid:sun".into())
            .unwrap();
        assert_eq!(g.exposed_ports[&key].icon, "fa6-solid:sun");

        // Unsafe icon (contains `<`) — rejected; previous value retained.
        let err = g
            .set_exposed_port_meta(
                &key,
                "Label2".into(),
                "Desc2".into(),
                "<script>x</script>".into(),
            )
            .unwrap_err();
        assert!(matches!(err, GraphError::InvalidIcon { .. }));
        assert_eq!(g.exposed_ports[&key].icon, "fa6-solid:sun");
        assert_eq!(g.exposed_ports[&key].label, "Label");
    }

    #[test]
    fn exposed_ports_round_trip_preserves_order() {
        let mut g = Graph::<TestWireKind>::new();
        let id = g.add_node("node", vec![scalar_in("a"), scalar_in("b"), scalar_in("c")]);
        g.expose_port(&id, "c").unwrap();
        g.expose_port(&id, "a").unwrap();
        g.expose_port(&id, "b").unwrap();
        let before: Vec<String> = g.exposed_ports.keys().cloned().collect();

        let json = serde_json::to_string(&g).unwrap();
        let back: Graph<TestWireKind> = serde_json::from_str(&json).unwrap();
        let after: Vec<String> = back.exposed_ports.keys().cloned().collect();
        assert_eq!(before, after);
    }
}
