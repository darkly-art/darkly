//! Levels node — clamp-and-rescale window over a 0..1 scalar.
//!
//! Maps `input` linearly from `[in_low, in_high]` onto `[0, 1]`, clamping
//! to the unit range. Simpler than [`super::curve`] (no spline LUT, no
//! UI editor) and complements [`super::remap`] (which has arbitrary
//! output ranges but always treats `[in_low, in_high]` as a window):
//! `levels` is the right tool when the *meaning* of the operation is
//! "open the shadow / blowout the highlight," not "general affine map."
//!
//! Squeezing `in_low` and `in_high` together gives a soft threshold —
//! the `1e-6` floor on the denominator keeps the divide stable even
//! when the two ends are equal, so a threshold node and a window node
//! are the same node in two different parameter regimes.
//!
//! Output capping (Photoshop's "output white") is intentionally **not**
//! a built-in slider — compose with [`super::multiply`] when you need
//! it (a `multiply.b` default of 0.6 caps brightness at 60%). One
//! primitive per operation, kept minimal.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "levels";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "modulate",
            display_name: "Levels",
            description: "Adjusts a value's low end, high end, and midpoint — the same idea as the image-editing Levels filter.",
            ports: vec![
                PortDef::input("input", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Input value (0\u{2013}1) to window"),
                PortDef::input("in_low", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.0)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Input black point — anything at or below this clips to 0"),
                PortDef::input("in_high", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_description(
                        "Input white point — anything at or above this saturates to 1",
                    ),
                PortDef::output("output", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Clamp((input - in_low) / (in_high - in_low), 0, 1)"),
            ],
            params: &[],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_fallback_icon: None,
        },
        || Box::new(LevelsEvaluator),
    )
}

pub struct LevelsEvaluator;

fn levels_apply(input: f32, in_low: f32, in_high: f32) -> f32 {
    let denom = (in_high - in_low).max(1e-6);
    ((input - in_low) / denom).clamp(0.0, 1.0)
}

impl BrushNodeEvaluator for LevelsEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let input = ctx.input_f32("input");
        let in_low = ctx.input_f32("in_low");
        let in_high = ctx.input_f32("in_high");
        vec![(
            "output".into(),
            ScalarValue::Scalar(levels_apply(input, in_low, in_high)),
        )]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("output") {
            return Ok(wgsl);
        }
        let input = cctx.input("input").as_f32();
        let in_low = cctx.input("in_low").as_f32();
        let in_high = cctx.input("in_high").as_f32();
        // 1e-6 floor stops the divide from exploding when the two
        // ends are equal — the formula degenerates into a hard step
        // around `in_low` in that case, which is the threshold mode.
        let expr = format!(
            "clamp((({input}) - ({in_low})) / max(({in_high}) - ({in_low}), 1e-6), 0.0, 1.0)"
        );
        wgsl.outputs.insert("output".into(), expr);
        Ok(wgsl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_defaults_pass_through() {
        // in_low=0, in_high=1 → output == input clamped to [0, 1].
        for &x in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let y = levels_apply(x, 0.0, 1.0);
            assert!((y - x).abs() < 1e-6, "x={x} → y={y}");
        }
    }

    #[test]
    fn clamps_out_of_range_inputs() {
        assert_eq!(levels_apply(-0.5, 0.0, 1.0), 0.0);
        assert_eq!(levels_apply(1.5, 0.0, 1.0), 1.0);
    }

    #[test]
    fn windowed_input_remaps_to_full_range() {
        // in_low=0.25, in_high=0.75: 0.25 → 0, 0.5 → 0.5, 0.75 → 1.
        assert!((levels_apply(0.25, 0.25, 0.75) - 0.0).abs() < 1e-6);
        assert!((levels_apply(0.5, 0.25, 0.75) - 0.5).abs() < 1e-6);
        assert!((levels_apply(0.75, 0.25, 0.75) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn threshold_mode_in_low_equals_in_high() {
        // When in_low == in_high we want a near-step function around
        // that pivot. The 1e-6 floor stabilises the divide so the
        // output is 0 below and 1 above (with a sub-pixel transition).
        let pivot = 0.4;
        assert_eq!(levels_apply(0.1, pivot, pivot), 0.0);
        assert_eq!(levels_apply(0.9, pivot, pivot), 1.0);
    }

    #[test]
    fn degenerate_inverted_range_clamps_to_zero() {
        // in_low > in_high → denominator floors at 1e-6, so for any
        // input <= in_low the numerator is <=0 and clamp yields 0.
        // Tests that we don't panic / produce NaN in the inverted case.
        assert_eq!(levels_apply(0.5, 0.8, 0.2), 0.0);
    }
}
