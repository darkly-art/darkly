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
pub struct NodeRegistration<W: WireKind> {
    /// Unique identifier (e.g. "pen_input", "multiply").
    pub type_id: &'static str,
    /// UI category (e.g. "sensor", "math", "gpu").
    pub category: &'static str,
    /// Human-readable name (e.g. "Pen Input", "Multiply").
    pub display_name: &'static str,
    /// Port definitions for this node type.
    pub ports: Vec<PortDef<W>>,
    /// Parameter definitions (for inline UI sliders).
    pub params: &'static [ParamDef],
    /// Whether this node requires GPU execution.
    pub is_gpu: bool,
}
