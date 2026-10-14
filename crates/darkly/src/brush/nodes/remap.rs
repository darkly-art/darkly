//! Map Range node: Scalar → Scalar.
//!
//! Maps `input` from `in_low..in_high` onto `out_low..out_high` affinely,
//! holding the result inside the output range unless `clamp` is off. With
//! the defaults (`0..1` onto `0..1`, clamp on) it is a Levels window: an
//! input black point and white point with everything outside pinned.
//!
//! Every port speaks the normalized `[0, 1]` signal range, so the wire
//! remap carries the result onto whatever range the downstream port
//! declares. `random -> remap (out 0.4..0.6) -> hsv.hue` narrows a
//! full-turn hue jitter to the middle fifth of the hue range, about 36
//! degrees either way.
//!
//! Squeezing `in_low` and `in_high` together gives a soft threshold: the
//! `1e-6` floor on the denominator keeps the divide stable when the two
//! ends are equal, so a threshold and a window are the same node in two
//! parameter regimes.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "remap";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "modulate",
            display_name: "Map Range",
            description: "Maps a value from one range onto another, with an optional clamp. With the defaults it is a Levels window.",
            ports: vec![
                PortDef::input("input", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_description("The value to map"),
                PortDef::input("in_low", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("In Low")
                    .with_description("Input low end: maps to Out Low"),
                PortDef::input("in_high", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("In High")
                    .with_description("Input high end: maps to Out High"),
                PortDef::input("out_low", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Out Low")
                    .with_description("Where In Low lands"),
                PortDef::input("out_high", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Out High")
                    .with_description("Where In High lands"),
                PortDef::input("clamp", BrushWireType::Bool)
                    .with_value(InputValue::Bool(true))
                    .with_label("Clamp")
                    .with_description(
                        "Hold the output inside Out Low..Out High instead of extrapolating past them",
                    ),
                PortDef::output("output", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("out_low + (input - in_low) / (in_high - in_low) * (out_high - out_low)"),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
        },
        || Box::new(RemapEvaluator),
    )
}

pub struct RemapEvaluator;

/// The map. The `1e-6` floor stops the divide from exploding when the two
/// input ends are equal: the formula degenerates into a hard step around
/// `in_low` in that case, which is the threshold mode.
fn remap_apply(
    input: f32,
    in_low: f32,
    in_high: f32,
    out_low: f32,
    out_high: f32,
    clamp: bool,
) -> f32 {
    let t = (input - in_low) / (in_high - in_low).max(1e-6);
    let t = if clamp { t.clamp(0.0, 1.0) } else { t };
    out_low + t * (out_high - out_low)
}

impl BrushNodeEvaluator for RemapEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let out = remap_apply(
            ctx.input_f32("input"),
            ctx.input_f32("in_low"),
            ctx.input_f32("in_high"),
            ctx.input_f32("out_low"),
            ctx.input_f32("out_high"),
            ctx.input_f32("clamp") >= 0.5,
        );
        vec![("output".into(), ScalarValue::Scalar(out))]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("output") {
            return Ok(wgsl);
        }
        let input = cctx.input("input").as_f32();
        let in_low = cctx.input("in_low").as_f32();
        let in_high = cctx.input("in_high").as_f32();
        let out_low = cctx.input("out_low").as_f32();
        let out_high = cctx.input("out_high").as_f32();
        let t = cctx.ident("remap_t");
        wgsl.body = format!(
            "    let {t} = (({input}) - ({in_low})) / max(({in_high}) - ({in_low}), 1e-6);\n"
        );
        // A literal `clamp` picks the branch at compile time; a wired one
        // selects per dab.
        let clamped = format!("clamp({t}, 0.0, 1.0)");
        let t_expr = match cctx.input("clamp").as_f32_literal() {
            Some(v) if v >= 0.5 => clamped,
            Some(_) => t.clone(),
            None => {
                let wired = cctx.input("clamp").as_f32();
                format!("select({t}, {clamped}, ({wired}) >= 0.5)")
            }
        };
        wgsl.outputs.insert(
            "output".into(),
            format!("(({out_low}) + {t_expr} * (({out_high}) - ({out_low})))"),
        );
        Ok(wgsl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window `levels` computed before `remap` replaced it.
    fn levels(input: f32, in_low: f32, in_high: f32) -> f32 {
        ((input - in_low) / (in_high - in_low).max(1e-6)).clamp(0.0, 1.0)
    }

    #[test]
    fn defaults_are_bit_identical_to_levels() {
        for (lo, hi) in [(0.0, 1.0), (0.25, 0.75), (0.4, 0.4), (0.8, 0.2)] {
            for i in -32..=96 {
                let x = i as f32 / 64.0;
                let want = levels(x, lo, hi);
                let got = remap_apply(x, lo, hi, 0.0, 1.0, true);
                assert_eq!(
                    got.to_bits(),
                    want.to_bits(),
                    "remap({x}, {lo}, {hi}) = {got}, levels = {want}"
                );
            }
        }
    }

    #[test]
    fn threshold_mode_in_low_equals_in_high() {
        // Equal ends: a step around the pivot, stabilised by the 1e-6 floor.
        let pivot = 0.4;
        assert_eq!(remap_apply(0.1, pivot, pivot, 0.0, 1.0, true), 0.0);
        assert_eq!(remap_apply(0.9, pivot, pivot, 0.0, 1.0, true), 1.0);
    }

    #[test]
    fn degenerate_inverted_range_clamps_to_zero() {
        // in_low > in_high: the denominator floors at 1e-6, so any input at
        // or below in_low clamps to 0 rather than going NaN.
        assert_eq!(remap_apply(0.5, 0.8, 0.2, 0.0, 1.0, true), 0.0);
    }

    #[test]
    fn output_range_maps() {
        assert!((remap_apply(0.5, 0.0, 1.0, 0.25, 0.75, true) - 0.5).abs() < 1e-6);
        assert!((remap_apply(1.0, 0.0, 1.0, 0.25, 0.75, true) - 0.75).abs() < 1e-6);
        assert!((remap_apply(0.0, 0.0, 1.0, 0.25, 0.75, true) - 0.25).abs() < 1e-6);
        // An inverted output range flips the signal.
        assert!((remap_apply(0.25, 0.0, 1.0, 1.0, 0.0, true) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn clamp_off_extrapolates() {
        assert!((remap_apply(1.5, 0.0, 1.0, 0.25, 0.75, false) - 1.0).abs() < 1e-6);
        assert!((remap_apply(1.5, 0.0, 1.0, 0.25, 0.75, true) - 0.75).abs() < 1e-6);
    }
}
