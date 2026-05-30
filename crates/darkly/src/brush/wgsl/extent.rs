//! Extent protocol — composition of per-node dab-bbox contributions.
//!
//! Every node declares an [`ExtentContribution`] describing how it grows
//! / clips the upstream dab bbox. [`compose_brush_extent`] walks the
//! execution plan in topological order and composes them into a single
//! `(factor, extra_px)` pair stored on
//! [`crate::brush::wgsl::CompiledBrush`]; the `paint` terminal multiplies
//! the per-dab effective radius by `factor` and adds `extra_px` to
//! produce the dab's `bbox_target_px`. That value is packed into the
//! per-dab record and read by the WGSL fragment shader to size the
//! rasterized quad and to clip the dab's write footprint to the layer
//! bbox.
//!
//! Because the value flows from the framework into both the CPU bbox
//! computation and the GPU shader (via the dab record), the CPU bbox
//! and shader write footprint cannot diverge.

use std::collections::{HashMap, HashSet};

use crate::brush::eval::BrushNodeEvaluator;
use crate::brush::wire::BrushWireType;
use crate::gpu::params::ParamValue;
use crate::nodegraph::{ExecutionPlan, NodeId, PortDef, PortDir};

/// One node's contribution to the per-brush dab bounding-box extent.
///
/// The bug this protocol was introduced to fix: the WGSL prelude inflated
/// the rasterized quad by a hardcoded `QUAD_R_MAX = 1.6` while the CPU
/// layer-clip bbox used the un-inflated `radius`. On a mid-stroke
/// save-point rewind, the save-point system cleared pixels outside the
/// CPU bbox but only restored into it — so anything the shader wrote in
/// the inflation margin was lost, visibly truncating previous dabs to a
/// smaller square as the user kept drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtentContribution {
    /// No effect — bbox passes through unchanged from upstream.
    Identity,
    /// Multiplier on upstream extent. `circle` uses `1 + amp_max` for
    /// sine/perlin (or the superformula's `r_max`) so the bbox covers
    /// the shape's worst-case rasterized footprint.
    Multiply(f32),
    /// Additive canvas-pixel padding on top of upstream. Future
    /// displacement / warp nodes use this (e.g. warp by ±strength px).
    /// `passthrough` multiplies the upstream extent; `added_px` is the
    /// post-multiply additive padding in canvas pixels.
    AddCanvasPixels { passthrough: f32, added_px: f32 },
    /// Hard cap below upstream — `bbox_target_px` is min'd with
    /// `factor * radius`. For clip-to-circle style masks.
    ClipTo(f32),
}

/// Per-node context passed to `BrushNodeEvaluator::extent`. Mirrors the
/// shape of [`crate::brush::wgsl::CompileWgslCtx`] minus the WGSL
/// plumbing: just port defs, params, and a wired-input set so
/// [`Self::port_max_value`] can pick the wire-aware max for each input.
pub struct ExtentCtx<'a> {
    pub node_id: NodeId,
    pub params: &'a [ParamValue],
    pub port_defs: &'a [PortDef<BrushWireType>],
    /// Names of input ports on this node that have an inbound wire.
    /// Used by [`Self::port_max_value`] to decide whether to return
    /// the port's `natural_range` max (wired) or its default (unwired).
    pub wired_inputs: HashSet<String>,
}

impl ExtentCtx<'_> {
    /// Maximum value the named input port can take, given the wire
    /// graph. For a wired input, returns the port's `natural_range`
    /// max (or its slider `max` if no natural range is declared) —
    /// the wire-boundary remap maps every wire to the dst's natural
    /// range, so that's the actual ceiling. For an unwired input,
    /// returns the port's `default` (the only value it can take).
    /// Unknown ports return `0.0`.
    pub fn port_max_value(&self, port_name: &str) -> f32 {
        let Some(port) = self
            .port_defs
            .iter()
            .find(|p| p.name == port_name && p.dir == PortDir::Input)
        else {
            return 0.0;
        };
        if self.wired_inputs.contains(port_name) {
            port.natural_range.map(|(_, max)| max).unwrap_or(port.max)
        } else {
            port.default
        }
    }
}

/// Compose every node's [`ExtentContribution`] into a single
/// `(factor, extra_px)` pair for the brush. Walks every step in the
/// execution plan in topological order; each node sees the upstream-
/// accumulated extent and contributes its own multiplier / additive
/// padding / clip. Nodes that don't override `extent` (the trait
/// default returns [`ExtentContribution::Identity`]) leave the running
/// pair unchanged.
pub(crate) fn compose_brush_extent(
    graph: &crate::nodegraph::Graph<BrushWireType>,
    plan: &ExecutionPlan,
    evaluators: &HashMap<String, Box<dyn BrushNodeEvaluator>>,
) -> (f32, f32) {
    let mut factor: f32 = 1.0;
    let mut extra_px: f32 = 0.0;
    for step in &plan.steps {
        let Some(evaluator) = evaluators.get(&step.type_id) else {
            continue;
        };
        let Some(node) = graph.nodes.get(&step.node_id) else {
            continue;
        };
        let wired_inputs: HashSet<String> = step
            .input_slots
            .iter()
            .map(|s| s.port_name.clone())
            .collect();
        let ectx = ExtentCtx {
            node_id: step.node_id,
            params: &node.params,
            port_defs: &node.ports,
            wired_inputs,
        };
        match evaluator.extent(&ectx) {
            ExtentContribution::Identity => {}
            ExtentContribution::Multiply(m) => {
                factor *= m;
                extra_px *= m;
            }
            ExtentContribution::AddCanvasPixels {
                passthrough,
                added_px,
            } => {
                factor *= passthrough;
                extra_px = extra_px * passthrough + added_px;
            }
            ExtentContribution::ClipTo(cap) => {
                factor = factor.min(cap);
            }
        }
    }
    (factor, extra_px)
}
