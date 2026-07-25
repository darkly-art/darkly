//! Brush Settings node — the apply-to-all-brushes knobs that aren't live
//! stylus data: base `size`, `stabilize`, `spacing`, `spacing_min_px`.
//!
//! These are read **out-of-band** by the engine at stroke start (not per
//! dab, not through the graph): `spacing`/`spacing_min_px` drive dab
//! placement, `stabilize` selects the stabilizer, and `size` is injected as
//! the ambient base size every terminal multiplies its per-touch modulation
//! onto. They live on their own node — rather than on `pen_input` — because
//! they are user settings, not stylus signals; `pen_input` stays pure live
//! sensor data.
//!
//! `size` is additionally a **settable-source** (`.source()`): any node can
//! wire from `brush_settings.size` to read the stroke's base size as a graph
//! signal, and the editor hides the source handle once the port is driven.
//! Because this is an ordinary CPU node (not special-cased in the runner like
//! `pen_input`), the runner's generic settable-source seeding publishes
//! `size`'s value on its slot every dab, and [`Self::compile_wgsl`] packs it
//! into a dab field when — and only when — a node consumes it.

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::spacing::SpacingConfig;
use crate::brush::wgsl::{CompileWgslCtx, DabField, NodeWgsl, WgslType};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{Graph, NodeId, NodeRegistration, PortDef, PortDir, UnitType};

pub const TYPE_ID: &str = "brush_settings";

/// Registration default for the `size` base knob — the effective brush size
/// when no brush overrides it. Graphs without a `brush_settings` node fall
/// back to this.
pub const DEFAULT_BASE_SIZE: f32 = 0.1;

/// Node id of the (first) `brush_settings` node in `graph`, if any. The
/// out-of-band knobs all live on this node, so engine, CLI, and tests resolve
/// it here rather than re-scanning by type.
pub fn node_id(graph: &Graph<BrushWireType>) -> Option<NodeId> {
    graph
        .nodes()
        .iter()
        .find(|(_, n)| n.type_id == TYPE_ID)
        .map(|(id, _)| id.clone())
}

/// Read a scalar input port default off the (first) `brush_settings` node in
/// `graph`. Returns `None` when no such node is present or the named port
/// isn't on it (e.g. a graph that predates the port).
pub fn read_scalar_input(graph: &Graph<BrushWireType>, port_name: &str) -> Option<f32> {
    for node in graph.nodes().values() {
        if node.type_id == TYPE_ID {
            for port in &node.ports {
                if port.name == port_name && port.dir == PortDir::Input {
                    return Some(port.value.as_f32());
                }
            }
        }
    }
    None
}

/// Base brush size for `graph`, read out-of-band from the `brush_settings`
/// node's `size` input-port default. Falls back to [`DEFAULT_BASE_SIZE`].
/// Stroke-constant: read once at stroke start and injected as ambient state
/// the terminals multiply their modulation onto.
pub fn base_size(graph: &Graph<BrushWireType>) -> f32 {
    read_scalar_input(graph, "size").unwrap_or(DEFAULT_BASE_SIZE)
}

/// Build the `SpacingConfig` the stroke engine should run with for `graph`,
/// reading the `brush_settings` node's `spacing` and `spacing_min_px` port
/// defaults. Falls back to `SpacingConfig::default()` for graphs that predate
/// either port. A `spacing_min_px` of 0 (the registration default) also falls
/// back — it means "use the ratio alone, with the absolute floor".
pub fn spacing_config(graph: &Graph<BrushWireType>) -> SpacingConfig {
    let default = SpacingConfig::default();
    let ratio = read_scalar_input(graph, "spacing").unwrap_or(default.ratio);
    let min_px_raw = read_scalar_input(graph, "spacing_min_px").unwrap_or(0.0);
    let min_px = if min_px_raw > 0.0 {
        min_px_raw
    } else {
        default.min_px
    };
    SpacingConfig { ratio, min_px }
}

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "input",
            display_name: "Brush Settings",
            description: "Overall brush settings that apply to the whole stroke: size, stabilization, and dab spacing.",
            ports: vec![
                // Base brush size — the knob the brush bar and `[`/`]` hotkeys
                // drive. Read out-of-band at stroke start and injected as
                // ambient state the terminals multiply their per-touch
                // modulation onto. Also a settable-source: any node can wire
                // from `brush_settings.size` to read the base size as a signal.
                // `preview_value` pins previews to a canonical size so scrubbing
                // it doesn't redraw the editor preview / cursor halo.
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 4.0, DEFAULT_BASE_SIZE)
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:up-right-and-down-left-from-center")
                    .with_label("Size")
                    .exposed()
                    .source()
                    .with_preview_value(DEFAULT_BASE_SIZE)
                    .with_description("Overall brush size"),
                // Stabilization strength — read at stroke start, not per-dab.
                // `preview_irrelevant_scrub`: the synthetic editor-preview
                // stroke is a pre-cooked Bezier through `PassThrough`, so a
                // stabilize scrub can't change its output; declaring it keeps
                // the preview cache valid instead of re-rendering for nothing.
                PortDef::input("stabilize", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:wave-square")
                    .with_label("Stabilize")
                    .preview_irrelevant_scrub()
                    .with_description(
                        "Stroke stabilization strength (0 = off, 100% = maximum smoothing)",
                    ),
                // Dab spacing — read at stroke start as a fraction of the dab
                // diameter. No `preview_value`: spacing visibly changes the
                // rendered stroke (dab density), so a scrub *should* re-render.
                PortDef::input("spacing", BrushWireType::Scalar)
                    .with_range(0.01, 1.0, 0.10)
                    .with_natural_range(0.01, 1.0)
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:grip-lines-vertical")
                    .with_label("Spacing")
                    .with_description(
                        "Distance between dabs as a fraction of dab diameter. \
                         10% is the paint default; warp/smudge brushes typically want 1\u{2013}5%. \
                         The single-pass WGSL-compiled brush pipeline keeps even 1% spacing within frame budget.",
                    ),
                // Absolute-pixel spacing floor. Effective spacing per dab is
                // `max(diameter × ratio, spacing_min_px, ABSOLUTE_MIN_SPACING_PX)`.
                // Set this above zero — and ratio small — to pin dab spacing in
                // canvas pixels regardless of brush size (liquify uses this so
                // its per-dab displacement stays size-invariant).
                PortDef::input("spacing_min_px", BrushWireType::Scalar)
                    .with_range(0.0, 64.0, 0.0)
                    .with_natural_range(0.0, 32.0)
                    .with_unit(UnitType::Pixels)
                    .with_icon("fa6-solid:ruler-horizontal")
                    .with_label("Spacing min (px)")
                    .with_description(
                        "Absolute-pixel floor for dab spacing. 0 = use the \
                         ratio above; non-zero pins spacing to at least \
                         this many canvas pixels regardless of brush size.",
                    ),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_fallback_icon: None,
        },
        || Box::new(BrushSettingsEvaluator),
    )
}

/// No-op evaluator. `size` is published on its slot by the runner's generic
/// settable-source seeding; `stabilize`/`spacing`/`spacing_min_px` are read
/// out-of-band and never flow through the graph.
pub struct BrushSettingsEvaluator;

impl BrushNodeEvaluator for BrushSettingsEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// Emit the `size` settable-source as a per-dab field so it reaches a
    /// compiled brush's fragment shader — but only when a node consumes it,
    /// so an untapped size costs nothing. The value is packed from the slot
    /// the runner's generic source-seeding wrote (the ambient base size).
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if cctx.consumed_outputs.contains("size") {
            let field_name = cctx.dab_field_name("size");
            let key = field_name.clone();
            wgsl.dab_fields.push(DabField {
                name: field_name.clone(),
                ty: WgslType::F32,
                pack: Arc::new(move |outputs, bytes| {
                    let v = outputs.get(&key).map(|s| s.as_f32()).unwrap_or(0.0);
                    bytes.extend_from_slice(bytemuck::bytes_of(&v));
                }),
            });
            wgsl.outputs
                .insert("size".into(), format!("d.{field_name}"));
        }
        Ok(wgsl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::input_value::InputValue;
    use crate::brush::spacing::ABSOLUTE_MIN_SPACING_PX;

    fn graph_with_settings(inputs: &[(&str, f32)]) -> Graph<BrushWireType> {
        let reg = register();
        let mut ports = reg.ports.clone();
        for (name, v) in inputs {
            ports
                .iter_mut()
                .find(|p| p.name == *name && p.dir == PortDir::Input)
                .unwrap_or_else(|| panic!("port {name} present"))
                .value = InputValue::Scalar(*v);
        }
        let mut graph = Graph::<BrushWireType>::new();
        graph.add_node(TYPE_ID, ports);
        graph
    }

    #[test]
    fn base_size_reads_the_size_knob() {
        let graph = graph_with_settings(&[("size", 0.42)]);
        assert!((base_size(&graph) - 0.42).abs() < 1e-6);
    }

    #[test]
    fn base_size_falls_back_without_settings_node() {
        let graph = Graph::<BrushWireType>::new();
        assert!((base_size(&graph) - DEFAULT_BASE_SIZE).abs() < 1e-6);
    }

    #[test]
    fn spacing_config_reads_scrubbed_value() {
        let graph = graph_with_settings(&[("spacing", 0.5)]);
        let cfg = spacing_config(&graph);
        assert!((cfg.ratio - 0.5).abs() < 1e-6);
        assert!((cfg.distance(100.0) - 50.0).abs() < 1e-6);
    }

    #[test]
    fn spacing_config_zero_min_px_falls_back_to_default() {
        let graph = graph_with_settings(&[("spacing", 0.1), ("spacing_min_px", 0.0)]);
        let cfg = spacing_config(&graph);
        assert!(cfg.min_px >= ABSOLUTE_MIN_SPACING_PX);
    }

    #[test]
    fn spacing_config_honors_min_px_override() {
        let graph = graph_with_settings(&[("spacing", 0.05), ("spacing_min_px", 8.0)]);
        let cfg = spacing_config(&graph);
        assert!((cfg.ratio - 0.05).abs() < 1e-6);
        assert!((cfg.min_px - 8.0).abs() < 1e-6);
        assert!((cfg.distance(50.0) - 8.0).abs() < 1e-6);
    }

    #[test]
    fn spacing_config_falls_back_when_settings_missing() {
        let graph = Graph::<BrushWireType>::new();
        let cfg = spacing_config(&graph);
        let default = SpacingConfig::default();
        assert!((cfg.ratio - default.ratio).abs() < 1e-6);
        assert!((cfg.min_px - default.min_px).abs() < 1e-6);
    }
}
