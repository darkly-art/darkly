use serde::Serialize;

use super::graph::PortDef;
use super::WireKind;
use crate::gpu::params::ParamDef;

/// Static metadata describing a node type in a particular domain.
///
/// Analogous to `VeilRegistration` / `ToolRegistration` — each node
/// module exports a `pub fn register() -> NodeRegistration<W>`.
///
/// Only `Serialize` — this struct contains `&'static` references and
/// is constructed at registration time, never deserialized.
#[derive(Clone, Debug, Serialize)]
#[serde(bound = "")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-export", ts(concrete(W = crate::brush::wire::BrushWireType), bound = "W: ts_rs::TS"))]
pub struct NodeRegistration<W: WireKind> {
    /// Unique identifier (e.g. "pen_input", "multiply").
    pub type_id: &'static str,
    /// UI category for the add-node palette — describes what the node *does*,
    /// not how it executes. Current values: "input", "math", "modulate",
    /// "color", "shape", "texture", "output", and "internal" (filtered out).
    pub category: &'static str,
    /// Human-readable name (e.g. "Pen Input", "Multiply").
    pub display_name: &'static str,
    /// Short, single-sentence description of what this node does — shown as
    /// the add-node menu tooltip. Should read as a noun-phrase or imperative
    /// fragment in painter vocabulary (never engine-internal terms like
    /// "scalar" or "fragment shader"); per-port detail goes on the ports
    /// themselves via `PortDef::with_description`.
    pub description: &'static str,
    /// Port definitions for this node type.
    pub ports: Vec<PortDef<W>>,
    /// Parameter definitions (for inline UI sliders).
    pub params: &'static [ParamDef],
    /// Whether this node requires GPU execution.
    pub is_gpu: bool,
    /// True for output terminals whose upstream graph fuses into a
    /// compiled WGSL fragment shader. The dispatch walk in the runner
    /// skips every upstream GPU node when one of these is present —
    /// their contribution lives inside the terminal's compiled shader,
    /// only the terminal itself runs to queue dabs and flush.
    pub is_terminal: bool,
    /// Whether this terminal honours erase mode (paint vs. erase).
    /// Defaults `true`; smear/displace terminals that sample existing
    /// pixels (smudge, watercolor, liquify) override to `false` so the
    /// brush-tool options bar hides the erase toggle.
    pub supports_erase: bool,
    /// Iconify icon shown in place of baked dab/stroke thumbnails for any
    /// brush whose graph contains this node. Set by nodes whose output
    /// depends on existing canvas content — stroking the flat preview
    /// background renders blank, so the picker shows this icon instead.
    pub preview_fallback_icon: Option<&'static str>,
}
