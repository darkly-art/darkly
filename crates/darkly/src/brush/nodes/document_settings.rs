//! Document Settings node: constants describing the document a stroke is
//! landing in, so a brush can size itself against the artwork rather than the
//! pixel grid.
//!
//! The one member today is `dpi_scale`, the document's resolution relative to
//! [`DEFAULT_DPI`]. Multiply an authored pixel number by it and that number
//! keeps its *physical* size on any canvas: a paper grain authored as 2.5 px
//! at the reference resolution renders 2.5 px on a 300 DPI document and 10 px
//! on a 1200 DPI one, and the two print identically. Because the denominator
//! is the same constant a fresh document starts at, `dpi_scale` is exactly
//! 1.0 until the artist changes the DPI, so wiring this node into a shipped
//! brush changes nothing about how it currently paints.
//!
//! **Admission rule**, so this does not become a junk drawer: a port belongs
//! here if and only if it is (a) a scalar derived from [`Document`] state
//! alone, (b) constant for the duration of a stroke, and (c) meaningful to a
//! brush without the brush knowing which document it is in. Canvas width and
//! height pass, and are the obvious next members; when they land they must
//! read [`crate::brush::wgsl::IntrinsicUniforms::canvas_size`], which already
//! carries them to the shader, rather than packing a second copy. Layer
//! count, active layer id and selection bounds all fail (b) or (c).
//!
//! Unlike [`super::paint_color`] and [`super::brush_settings`], this node is
//! *not* seeded bespoke and *not* skipped in `execute_cpu`: it publishes from
//! a plain `evaluate_cpu`, so `build_slot_outputs` picks the value up
//! generically and [`CompileWgslCtx::uniform_field_name`] finds it in the
//! uniform packer with no central edit.
//!
//! [`Document`]: crate::document::Document

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl, UniformField, WgslType};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::document::DEFAULT_DPI;
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
                        "Document resolution relative to the 300 DPI reference. Multiply a \
                         pixel-sized value by this and it keeps its physical size on any canvas.",
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
        vec![(
            "dpi_scale".into(),
            ScalarValue::Scalar(ctx.dpi() / DEFAULT_DPI),
        )]
    }

    /// The document's resolution is constant for every dab, so it goes into
    /// the uniform buffer (one copy per stroke), not the per-dab record. Only
    /// emitted if a downstream node consumes it, so an unwired node costs
    /// nothing.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("dpi_scale") {
            return Ok(wgsl);
        }
        let field_name = cctx.uniform_field_name("dpi_scale");
        let key = field_name.clone();
        wgsl.uniform_fields.push(UniformField {
            name: field_name.clone(),
            ty: WgslType::F32,
            pack: Arc::new(move |outputs, bytes| {
                let v = outputs.get(&key).map(|s| s.as_f32()).unwrap_or(1.0);
                bytes.extend_from_slice(bytemuck::bytes_of(&v));
            }),
        });
        wgsl.outputs
            .insert("dpi_scale".into(), format!("u.{field_name}"));
        Ok(wgsl)
    }
}
