//! HSV Adjust node: Vec4 → Vec4.
//!
//! Rotates a colour's hue and scales its saturation and value: the
//! brush-graph twin of the Hue/Saturation image effect, sharing its WGSL
//! (`shaders/lib/hsv_adjust.wgsl`, prepended to every compiled brush) and
//! its CPU math ([`crate::color`]). Wire sensors into the deltas for
//! pressure-darkening or speed-fading, or `random` into `hue` for colour
//! jitter.
//!
//! `hue` is a rotation in degrees, as the effect's parameter is, and wraps
//! through 360. All three deltas declare bipolar natural ranges (`hue`
//! `[-180, 180]`, `saturation` and `value` `[-1, 1]`), so a `[0, 1]` source
//! such as `random` or a curve lands centered on "no change" and spans the
//! port's whole range: `random -> hue` is a full-turn jitter, and a `remap`
//! in between narrows it.
//!
//! The colour wire is straight (non-premultiplied) RGBA, which is what
//! `paint_color` emits and `stamp.color` consumes; alpha passes through.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::color::hsv_adjust_rgba;
use crate::nodegraph::{NodeRegistration, PortDef};
use crate::units::UnitType;

pub const TYPE_ID: &str = "hsv";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "color",
            display_name: "HSV Adjust",
            description: "Shifts a color's hue and scales its saturation and value, the brush-graph twin of the Hue/Saturation filter.",
            ports: vec![
                PortDef::input("color", BrushWireType::Vec4)
                    .with_value(InputValue::Vec4([1.0, 1.0, 1.0, 1.0]))
                    .with_description("The color to adjust (straight RGBA)"),
                PortDef::input("hue", BrushWireType::Scalar)
                    .with_range(-180.0, 180.0, 0.0)
                    .with_natural_range(-180.0, 180.0)
                    .with_unit(UnitType::Raw)
                    .with_label("Hue")
                    .with_description("Hue rotation in degrees, wrapping through 360"),
                PortDef::input("saturation", BrushWireType::Scalar)
                    .with_range(-1.0, 1.0, 0.0)
                    .with_natural_range(-1.0, 1.0)
                    .with_unit(UnitType::Percent)
                    .with_label("Saturation")
                    .with_description("Pushes the color toward grey (-100%) or full intensity (+100%)"),
                PortDef::input("value", BrushWireType::Scalar)
                    .with_range(-1.0, 1.0, 0.0)
                    .with_natural_range(-1.0, 1.0)
                    .with_unit(UnitType::Percent)
                    .with_label("Value")
                    .with_description("Darkens (-100%) or lightens (+100%) without changing hue"),
                PortDef::output("output", BrushWireType::Vec4)
                    .with_description("The adjusted color (straight RGBA)"),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
        },
        || Box::new(HsvEvaluator),
    )
}

pub struct HsvEvaluator;

impl BrushNodeEvaluator for HsvEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let c = ctx.input("color").as_color();
        let out = hsv_adjust_rgba(
            c,
            ctx.input_f32("hue"),
            ctx.input_f32("saturation"),
            ctx.input_f32("value"),
        );
        vec![("output".into(), ScalarValue::Vec4(out))]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("output") {
            return Ok(wgsl);
        }
        // Bind the input once so the upstream expression is emitted at one
        // site, however many times the output is consumed.
        let color_expr = cctx.input("color").as_vec4();
        let var = cctx.ident("hsv_c");
        wgsl.body = format!("    let {var} = {color_expr};\n");
        let hue = cctx.input("hue").as_f32();
        let saturation = cctx.input("saturation").as_f32();
        let value = cctx.input("value").as_f32();
        wgsl.outputs.insert(
            "output".into(),
            format!(
                "vec4<f32>(hsv_adjust({var}.rgb, ({hue}), ({saturation}), ({value})), {var}.a)"
            ),
        );
        Ok(wgsl)
    }
}
