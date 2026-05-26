//! Stamp dab GPU node — compile-only.
//!
//! Inlines `color × mask × flow` into the brush's compiled WGSL via
//! [`compile_wgsl`]. The upstream `tip` input is a scalar coverage
//! expression (typically from `circle.texture`'s compile output); the
//! emitted `dab` output is premultiplied RGBA that downstream paint
//! terminals consume.
//!
//! ## AlphaMask only
//!
//! The compiled path supports AlphaMask mode exclusively. The original
//! ImageStamp / LightnessMap / GradientMap modes relied on sampling a
//! real RGBA tip texture and were dropped together with the dispatch
//! pipeline in phase 4 of the compiled-port migration. Setting
//! `application` to any non-zero value makes brush load fail with a
//! clear error.
//!
//! ## Ignored ports
//!
//! `size_input`, `size`, `rotation`, `rotation_input`, `mirror_*`, and
//! `ratio` are no-ops in compiled mode — dab dimensions, rotation and
//! mirroring are owned by the terminal (which sizes its quad from
//! `bbox_radius`). Wiring them has no effect on the rendered dab. A
//! future revision can reintroduce rotation/mirror by rotating
//! `local_uv` before the shape evaluator runs.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl_compile::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![],
        node: NodeRegistration {
            type_id: "stamp",
            category: "shape",
            display_name: "Stamp Tip",
            ports: vec![
                PortDef::input("tip", BrushWireType::Texture)
                    .with_description("Brush tip image (scalar coverage in compiled mode)"),
                PortDef::input("size_input", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size Input")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-circle")
                    .with_description(
                        "Per-touch size multiplier. Ignored in compiled mode — \
                         the terminal owns the dab dimensions.",
                    ),
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 4.0, 0.1)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                    .exposed()
                    .with_preview_value(0.1)
                    .with_description("Overall brush size (ignored — terminal owns size)"),
                PortDef::input("rotation_input", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Rotation Input")
                    .with_unit(UnitType::Degrees)
                    .with_icon("fa-solid fa-rotate")
                    .with_description("Per-dab rotation (ignored in compiled mode)"),
                PortDef::input("rotation", BrushWireType::Scalar)
                    .with_range(-std::f32::consts::TAU, std::f32::consts::TAU, 0.0)
                    .with_label("Rotation")
                    .with_unit(UnitType::Degrees)
                    .with_icon("fa-solid fa-rotate")
                    .persist_in_thumbnail()
                    .with_description("Static rotation (ignored in compiled mode)"),
                PortDef::input("mirror_x", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Flip horizontally (ignored in compiled mode)"),
                PortDef::input("mirror_y", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Flip vertically (ignored in compiled mode)"),
                PortDef::input("ratio", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Ratio")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-arrows-left-right")
                    .with_description("Aspect ratio (ignored in compiled mode)"),
                PortDef::input("flow", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Flow")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa-solid fa-droplet")
                    .exposed()
                    .with_description("Paint deposited per dab"),
                PortDef::input("color", BrushWireType::Color).with_description("Brush color"),
                PortDef::output("dab", BrushWireType::Texture)
                    .with_description("The stamped brush mark (premultiplied RGBA)"),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in pixels (unused in compiled mode)"),
                PortDef::output("preview", BrushWireType::Texture)
                    .with_description("Brush preview (stubbed during phase 4 migration)"),
            ],
            params: &[
                // Enum stored as Int. Only `Alpha Mask` (= 0) is
                // supported on the compiled path; the other modes are
                // listed here so old brush JSON deserializes without a
                // schema mismatch, but `compile_wgsl` errors on any
                // non-zero value.
                ParamDef::Enum {
                    name: "application",
                    options: &["Alpha Mask", "Image Stamp", "Lightness Map", "Gradient Map"],
                    default: 0,
                },
            ],
            is_gpu: true,
        },
    }
}

pub struct StampEvaluator;

impl BrushNodeEvaluator for StampEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// Inline `color × mask × flow` into the brush's compiled WGSL.
    /// `tip` carries the upstream scalar coverage expression; the
    /// emitted `dab` output is premultiplied RGBA. Errors on any
    /// application mode other than AlphaMask — those relied on
    /// sampling a real RGBA tip texture and were dropped together
    /// with the dispatch pipeline.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("dab") {
            return Ok(wgsl);
        }
        let application = match cctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => *v,
            _ => 0,
        };
        if application != 0 {
            return Err(format!(
                "stamp.application = {application} (expected AlphaMask = 0); \
                 ImageStamp / LightnessMap / GradientMap were dropped in the \
                 compiled-port migration. Use a procedural tip (circle node) \
                 instead.",
            ));
        }
        let mask = cctx.input("tip").as_f32();
        let color = cctx.input("color").as_vec4();
        let flow = cctx.input("flow").as_f32();

        let fn_name = cctx.ident("stamp");
        let decls = format!(
            "fn {fn_name}(mask: f32, color: vec4<f32>, flow: f32) -> vec4<f32> {{\n\
             \x20   let a = color.a * mask * flow;\n\
             \x20   return vec4<f32>(color.rgb * a, a);\n\
             }}\n"
        );
        wgsl.decls = decls;
        wgsl.outputs.insert(
            "dab".into(),
            format!("{}({}, {}, {})", fn_name, mask, color, flow),
        );
        Ok(wgsl)
    }
}
