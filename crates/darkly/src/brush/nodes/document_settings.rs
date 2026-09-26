//! Document Settings node: constants describing the document a stroke is
//! landing in, so a brush can size itself against the artwork rather than the
//! pixel grid.
//!
//! The one member today is `dpi_scale`: the document's DPI over
//! [`REFERENCE_DPI`], which is the same reference-to-canvas factor the brush
//! runner applies at the boundary. It is exactly 1.0 on a reference-sized
//! document.
//!
//! Because every `UnitType::Pixels` port is *already* a reference-pixel
//! length that the runner converts, an authored pixel number already keeps
//! its physical size with no wiring at all. So the useful uses of this port
//! are the inverse ones: divide a reference length by `dpi_scale` to pin it
//! to the canvas pixel grid instead of the artwork, or scale a `Raw`
//! quantity that carries no unit of its own and therefore never crosses the
//! boundary.
//!
//! **Admission rule**, so this does not become a junk drawer: a port belongs
//! here if and only if it is (a) a scalar derived from [`Document`] state
//! alone, (b) constant for the duration of a stroke, and (c) meaningful to a
//! brush without the brush knowing which document it is in. Canvas width and
//! height pass, and are the obvious next members; when they land they must
//! read [`crate::brush::wgsl::IntrinsicUniforms::canvas_size`], which already
//! carries them to the shader, rather than packing a second copy. That is
//! exactly what `dpi_scale` does with
//! [`crate::brush::wgsl::IntrinsicUniforms::dpi_factor`]. Layer
//! count, active layer id and selection bounds all fail (b) or (c).
//!
//! Unlike [`super::paint_color`] and [`super::brush_settings`], this node is
//! *not* seeded bespoke and *not* skipped in `execute_cpu`: it publishes from
//! a plain `evaluate_cpu`, so `build_slot_outputs` picks the value up
//! generically and [`CompileWgslCtx::uniform_field_name`] finds it in the
//! uniform packer with no central edit.
//!
//! [`Document`]: crate::document::Document
//! [`REFERENCE_DPI`]: crate::document::REFERENCE_DPI

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub const TYPE_ID: &str = "document_settings";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "input",
            display_name: "Document Settings",
            description: "Constants describing the document itself, so a brush can size \
                          itself against the artwork instead of the pixel grid.",
            ports: vec![
                // No `natural_range`: this is a ratio, not a normalized
                // signal, so `apply_wire_remap` must pass it through raw. A
                // declared range here would silently rescale
                // `dpi_scale -> noise.scale` against that port's `1 .. 512`
                // and produce nonsense. Same reasoning as `pen_input`'s
                // `distance` / `time` / `tilt_direction`.
                PortDef::output("dpi_scale", BrushWireType::Scalar)
                    .with_unit(UnitType::Raw)
                    .with_description(
                        "The document's DPI divided by the reference DPI (100). Pixel-sized \
                         ports already keep their physical size on any document; divide by \
                         this to pin a length to the canvas pixel grid instead.",
                    ),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
        },
        || Box::new(DocumentSettingsEvaluator),
    )
}

pub struct DocumentSettingsEvaluator;

impl BrushNodeEvaluator for DocumentSettingsEvaluator {
    /// Publishes on its output slot like any other CPU node. The CPU value is
    /// load-bearing, not decorative: a DPI-driven size reaches `dab_size`,
    /// which the stroke engine reads for spacing and save-point bboxes.
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![("dpi_scale".into(), ScalarValue::Scalar(ctx.dpi_factor()))]
    }

    /// The shader already carries this number: the sample-coordinate emitter
    /// needs it at the GPU boundary, so it rides in
    /// [`crate::brush::wgsl::IntrinsicUniforms::dpi_factor`]. This node reads
    /// that field rather than packing a parallel uniform of its own, which
    /// would be the same fact in two places. Nothing to emit, so an unwired
    /// node costs nothing either way.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("dpi_scale") {
            return Ok(wgsl);
        }
        wgsl.outputs
            .insert("dpi_scale".into(), "u.intrinsic.dpi_factor".into());
        Ok(wgsl)
    }
}
