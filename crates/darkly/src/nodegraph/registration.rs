use serde::Serialize;

use super::graph::PortDef;
use super::WireKind;
use crate::catalog::CatalogEntry;

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
    /// "color", "shape", "texture", "output". Nothing filters on it; every
    /// registered node appears in the palette and in the catalog.
    pub category: &'static str,
    /// Human-readable name (e.g. "Pen Input", "Multiply").
    pub display_name: &'static str,
    /// Short, single-sentence description of what this node does — shown as
    /// the add-node menu tooltip. Should read as a noun-phrase or imperative
    /// fragment in painter vocabulary (never engine-internal terms like
    /// "scalar" or "fragment shader"); per-port detail goes on the ports
    /// themselves via `PortDef::with_description`.
    pub description: &'static str,
    /// Port definitions for this node type — the node's single, unified
    /// input/output list. Every input carries its own authored value and
    /// widget metadata on the [`PortDef`]; there is no separate parameter
    /// system.
    pub ports: Vec<PortDef<W>>,
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

impl<W: WireKind> NodeRegistration<W> {
    /// This node type as one browsable catalog entry.
    ///
    /// Lives here rather than on a per-domain wrapper so a second node domain
    /// gets its catalog for free — the domain contributes only the catalog's
    /// identity.
    ///
    /// Ports are deliberately not projected into `params`: a port carries a
    /// direction and a wire type, and
    /// [`ParamInfo`](crate::engine::types::ParamInfo) has room for neither, so
    /// the projection would present a node's outputs as settable parameters.
    /// Documenting ports wants the registration serialized whole, which it
    /// already can be, not flattened into the wrong shape.
    ///
    /// No icon: `preview_fallback_icon` is a substitute for a brush preview
    /// that cannot be baked, not a palette glyph, and only four node types
    /// declare one.
    pub fn catalog_entry(&self) -> CatalogEntry {
        CatalogEntry::new(self.type_id, self.display_name)
            .with_description(self.description)
            .with_category(self.category)
    }
}
