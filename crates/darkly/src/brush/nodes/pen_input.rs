//! Pen Input sensor node — source of all tablet data.
//!
//! Outputs 14 sensor values.  This node is special-cased in the runner:
//! `seed_sensors()` writes directly to its output slots (no virtual
//! dispatch).  The evaluator is a no-op.

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::spacing::SpacingConfig;
use crate::brush::wgsl::{CompileWgslCtx, DabField, NodeWgsl, WgslType};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{Graph, NodeRegistration, PortDef, PortDir, UnitType};

pub const TYPE_ID: &str = "pen_input";

/// Read a scalar input port default off the (first) pen_input node in `graph`.
/// Returns `None` when no pen_input node is present or the named port isn't
/// on it (e.g. an older brush from before the port was added).
pub fn read_scalar_input(graph: &Graph<BrushWireType>, port_name: &str) -> Option<f32> {
    for node in graph.nodes().values() {
        if node.type_id == TYPE_ID {
            for port in &node.ports {
                if port.name == port_name && port.dir == PortDir::Input {
                    return Some(port.default);
                }
            }
        }
    }
    None
}

/// Build the `SpacingConfig` the stroke engine should run with for `graph`,
/// reading the pen_input node's `spacing` and `spacing_min_px` port defaults.
/// Falls back to `SpacingConfig::default()` for graphs that predate either
/// port. A `spacing_min_px` of 0 (the registration default) also falls back —
/// it means "use the ratio alone, with the absolute floor".
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
            display_name: "Pen Input",
            description: "Live data from the stylus: pressure, tilt, velocity, and position along the stroke.",
            ports: vec![
            PortDef::output("pressure", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description("Pen pressure (0 = no pressure, 1 = full pressure)"),
            PortDef::output("x_tilt", BrushWireType::Scalar)
                .with_natural_range(-1.0, 1.0)
                .with_description("Horizontal tilt of the pen barrel (-1 = left, 1 = right)"),
            PortDef::output("y_tilt", BrushWireType::Scalar)
                .with_natural_range(-1.0, 1.0)
                .with_description("Vertical tilt of the pen barrel (-1 = toward user, 1 = away)"),
            PortDef::output("tilt_magnitude", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description(
                    "How far the pen is tilted from vertical (0 = upright, 1 = flat)",
                ),
            // No `natural_range`: radians are a unit, not a normalized
            // signal. A wire from `tilt_direction → stamp.rotation` (both
            // in radians) is a unit-preserving identity. Users who want a
            // normalized angle can pre-scale through `multiply` or `curve`.
            PortDef::output("tilt_direction", BrushWireType::Scalar)
                .with_description(
                    "Compass direction of pen tilt in radians (0 = right, π/2 = down)",
                ),
            PortDef::output("rotation", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description("Barrel rotation of the pen around its own axis (0\u{2013}1)"),
            PortDef::output("tangential_pressure", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description("Pressure on the pen's side wheel/slider (Wacom Airbrush)"),
            PortDef::output("speed", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description("Stroke speed in pixels per second, normalized"),
            // `distance`, `time`, and `index` are intentionally without a
            // `natural_range`: they're unbounded cumulative counters and
            // there is no meaningful upper end to remap against. Wires from
            // them pass through raw — feed them into a `clamp` or `remap`
            // node first if you need them in a bounded domain.
            PortDef::output("distance", BrushWireType::Scalar)
                .with_description("Cumulative distance traveled along the stroke (pixels)"),
            // No `natural_range`: radians are a unit. `drawing_angle →
            // stamp.rotation` is the canonical use case (brush faces the
            // stroke) and it must pass radians through unchanged.
            PortDef::output("drawing_angle", BrushWireType::Scalar)
                .with_description("Direction of motion along the stroke in radians (0 = right, π/2 = down). Wire to `stamp.rotation` for brushes that face the stroke."),
            PortDef::output("time", BrushWireType::Scalar)
                .with_description("Elapsed time since the stroke began (seconds)"),
            PortDef::output("position", BrushWireType::Vec2)
                .with_description("Current cursor position in canvas coordinates (x, y)"),
            PortDef::output("motion", BrushWireType::Vec2).with_description(
                "Per-dab motion vector in canvas pixels (delta from previous sample)",
            ),
            PortDef::output("index", BrushWireType::Int)
                .with_description("Dab index within the current stroke (0, 1, 2, ...)"),
            PortDef::output("fade", BrushWireType::Scalar)
                .with_natural_range(0.0, 1.0)
                .with_description("Stroke fade-out (0 at start, 1 at stroke end)"),
            // Stabilization strength — input port read at stroke start,
            // not per-dab.  Exposed via the eye toggle like any other port.
            //
            // `preview_irrelevant_scrub` marks this port as not affecting
            // editor-preview output: the synthetic preview stroke is a
            // pre-cooked Bezier rendered through `PassThrough` regardless
            // of the graph's stabilize value. Declaring it here keeps the
            // editor preview's cache valid when the user scrubs stabilize,
            // instead of re-rendering a full stroke for no visible effect.
            PortDef::input("stabilize", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_natural_range(0.0, 1.0)
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-wave-square")
                .with_label("Stabilize")
                .preview_irrelevant_scrub()
                .with_description(
                    "Stroke stabilization strength (0 = off, 100% = maximum smoothing)",
                ),
            // Dab spacing — read at stroke start as a fraction of the dab
            // diameter. Like `stabilize`, this is brush-level config that
            // currently lives here because the engine reads pen_input port
            // defaults out-of-band; both move together when the brush
            // settings bar gets redesigned.
            //
            // No `preview_value` — spacing visibly affects the rendered
            // stroke (dab density along the path) and a spacing scrub
            // *should* re-render the preview.
            PortDef::input("spacing", BrushWireType::Scalar)
                .with_range(0.01, 1.0, 0.10)
                .with_natural_range(0.01, 1.0)
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-grip-lines-vertical")
                .with_label("Spacing")
                .with_description(
                    "Distance between dabs as a fraction of dab diameter. \
                     10% is the paint default; warp/smudge brushes typically want 1\u{2013}5%. \
                     The single-pass WGSL-compiled brush pipeline keeps even 1% spacing within frame budget.",
                ),
            // Absolute-pixel spacing floor. The effective spacing per
            // dab is `max(diameter × ratio, spacing_min_px,
            // ABSOLUTE_MIN_SPACING_PX)`. Set this above zero — and
            // ratio to a small value — to pin dab spacing in canvas
            // pixels regardless of brush size. Liquify uses this so
            // its per-dab displacement (= strength × spacing) stays
            // size-invariant: the strength slider names a fixed pixel
            // amount, not a fraction of brush radius.
            PortDef::input("spacing_min_px", BrushWireType::Scalar)
                .with_range(0.0, 64.0, 0.0)
                .with_natural_range(0.0, 32.0)
                .with_unit(UnitType::Pixels)
                .with_icon("fa-solid fa-ruler-horizontal")
                .with_label("Spacing min (px)")
                .with_description(
                    "Absolute-pixel floor for dab spacing. 0 = use the \
                     ratio above; non-zero pins spacing to at least \
                     this many canvas pixels regardless of brush size.",
                ),
        ],
        params: &[],
        is_gpu: false,
        is_terminal: false,
        supports_erase: true,
    },
    || Box::new(PenInputEvaluator),
    )
}

/// No-op evaluator — `seed_sensors()` handles this node directly.
pub struct PenInputEvaluator;

impl BrushNodeEvaluator for PenInputEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        // Slots are written by seed_sensors(), not by the evaluator.
        vec![]
    }

    /// Pen-input outputs become per-dab record fields. The compiled
    /// `paint` terminal reads the slot table after
    /// `execute_cpu` and packs the values into the dab record using
    /// the lookup-by-name keys this method registers.
    ///
    /// Only emits dab fields for outputs that are *consumed* by some
    /// downstream node (per `cctx.consumed_outputs`). Unwired pen
    /// ports cost nothing. Fields are declared alignment-descending
    /// within the node's contribution so the std430 layout has no
    /// internal gaps.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();

        // vec2 outputs first (alignment 8).
        let vec2_outputs = ["position", "motion"];
        for name in &vec2_outputs {
            if !cctx.consumed_outputs.contains(*name) {
                continue;
            }
            let field_name = cctx.dab_field_name(name);
            let key = field_name.clone();
            wgsl.dab_fields.push(DabField {
                name: field_name.clone(),
                ty: WgslType::Vec2,
                pack: Arc::new(move |outputs, bytes| {
                    let v = outputs.get(&key).map(|s| s.as_vec2()).unwrap_or([0.0; 2]);
                    bytes.extend_from_slice(bytemuck::bytes_of(&v));
                }),
            });
            wgsl.outputs
                .insert((*name).into(), format!("d.{}", field_name));
        }

        // f32-equivalent outputs (alignment 4). Includes scalar sensors
        // and the `index` int (emitted as f32 — fine for the first
        // ~16M dabs, cast to u32 in WGSL where downstream nodes want it).
        let f32_outputs = [
            "pressure",
            "x_tilt",
            "y_tilt",
            "tilt_magnitude",
            "tilt_direction",
            "rotation",
            "tangential_pressure",
            "speed",
            "distance",
            "drawing_angle",
            "time",
            "index",
            "fade",
        ];
        for name in &f32_outputs {
            if !cctx.consumed_outputs.contains(*name) {
                continue;
            }
            let field_name = cctx.dab_field_name(name);
            let key = field_name.clone();
            wgsl.dab_fields.push(DabField {
                name: field_name.clone(),
                ty: WgslType::F32,
                pack: Arc::new(move |outputs, bytes| {
                    let v = outputs.get(&key).map(|s| s.as_f32()).unwrap_or(0.0);
                    bytes.extend_from_slice(bytemuck::bytes_of(&v));
                }),
            });
            // `drawing_angle` is `atan2(canvas_dy, canvas_dx)` — a
            // canvas-frame angle. The skeleton subtracts `view_rotation`
            // from `theta`, so to keep `pen.drawing_angle → circle.rotation_input`
            // (the canonical stroke-follow wire) aligned with the on-
            // screen stroke direction, subtract `view_rotation` here too:
            // canvas-frame angle minus V = screen-frame angle. Both
            // adjustments compose into screen-relative stamp orientation
            // for static knobs AND screen-aligned stroke following.
            let expr = if *name == "drawing_angle" {
                format!("(d.{} - u.intrinsic.view_rotation)", field_name)
            } else {
                format!("d.{}", field_name)
            };
            wgsl.outputs.insert((*name).into(), expr);
        }

        Ok(wgsl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::spacing::ABSOLUTE_MIN_SPACING_PX;

    fn graph_with_pen_input(
        spacing: Option<f32>,
        spacing_min_px: Option<f32>,
    ) -> Graph<BrushWireType> {
        let reg = register();
        let mut ports = reg.ports.clone();
        if let Some(v) = spacing {
            ports
                .iter_mut()
                .find(|p| p.name == "spacing")
                .expect("spacing port present")
                .default = v;
        }
        if let Some(v) = spacing_min_px {
            ports
                .iter_mut()
                .find(|p| p.name == "spacing_min_px")
                .expect("spacing_min_px port present")
                .default = v;
        }
        let mut graph = Graph::<BrushWireType>::new();
        graph.add_node(TYPE_ID, ports, vec![]);
        graph
    }

    /// Regression: scrubbing the pen_input "spacing" port must change the
    /// SpacingConfig the preview renderer feeds to the stroke engine.
    /// Previously the editor preview hardcoded `SpacingConfig::default()`,
    /// so spacing scrubs produced no visible change in the preview.
    #[test]
    fn spacing_config_reads_scrubbed_value() {
        let graph = graph_with_pen_input(Some(0.5), None);
        let cfg = spacing_config(&graph);
        assert!((cfg.ratio - 0.5).abs() < 1e-6, "got ratio {}", cfg.ratio);
        // Distance scales with the user-set ratio, not the 10% default.
        assert!((cfg.distance(100.0) - 50.0).abs() < 1e-6);
    }

    /// `spacing_min_px = 0` is the registration default and means
    /// "use the ratio alone, with the absolute floor". Must NOT clobber
    /// the spacing engine's default min_px down to zero.
    #[test]
    fn spacing_config_zero_min_px_falls_back_to_default() {
        let graph = graph_with_pen_input(Some(0.1), Some(0.0));
        let cfg = spacing_config(&graph);
        assert!(cfg.min_px >= ABSOLUTE_MIN_SPACING_PX);
    }

    /// A non-zero `spacing_min_px` scrub must pin the absolute spacing
    /// floor — Liquify and similar brushes rely on this.
    #[test]
    fn spacing_config_honors_min_px_override() {
        let graph = graph_with_pen_input(Some(0.05), Some(8.0));
        let cfg = spacing_config(&graph);
        assert!((cfg.ratio - 0.05).abs() < 1e-6);
        assert!((cfg.min_px - 8.0).abs() < 1e-6);
        // 50px diameter × 5% = 2.5px, clamped up to the 8px min.
        assert!((cfg.distance(50.0) - 8.0).abs() < 1e-6);
    }

    /// Graphs that predate either port (older saved brushes) fall back
    /// cleanly to the engine's defaults instead of e.g. zeroing the ratio.
    #[test]
    fn spacing_config_falls_back_when_pen_input_missing() {
        let graph = Graph::<BrushWireType>::new();
        let cfg = spacing_config(&graph);
        let default = SpacingConfig::default();
        assert!((cfg.ratio - default.ratio).abs() < 1e-6);
        assert!((cfg.min_px - default.min_px).abs() < 1e-6);
    }
}
