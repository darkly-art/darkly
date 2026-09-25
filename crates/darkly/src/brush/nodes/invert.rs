//! Invert node: `output = 1 - input`.
//!
//! The wire-side form of a brush-bar entry's `invert` flag. That flag mirrors
//! one control's reading, but it is per-entry state on a single `(node, port)`
//! pair, so a `user_input` dial fanned out to several sinks is either inverted
//! for all of them or for none. This node inverts one wire, so an author can
//! route the same dial straight into sink A and through here into sink B.
//!
//! The complement is taken in unit space and left unclamped. Both ports
//! declare a `0..1` natural range, so the wire remap normalizes a bipolar
//! source such as `pen_input.x_tilt` (`-1..1`) before the complement and
//! rescales the result into the sink's range when the sink declares one. Into
//! a range-less sink (`add.a`, `multiply.a`) the output is the unit-normalized
//! complement: a tilt of `0.5` arrives as `0.25`. A `user_input` dial declares
//! no natural range, so its value passes raw; on its default `0..1` slider the
//! node reads exactly as the entry flag would.
//!
//! The input defaults to `0.0` like `curve.input` and `levels.input`: the port
//! is meant to be wired, and an unwired inverter reading `1.0` is the honest
//! complement of that default. `0.5` is the one value `1 - x` leaves alone, and
//! would make an unwired node inert, but at the cost of a default unlike every
//! other scalar reshaper for no author-visible gain.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "invert";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "modulate",
            display_name: "Invert",
            description: "Flips a value end for end: 0 becomes 1 and 1 becomes 0. Put it on \
                          the wires from one dial that should read backwards for some sinks \
                          but not others.",
            ports: vec![
                PortDef::input("input", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Value to invert (0-1)"),
                PortDef::output("output", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("1 - input"),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
        },
        || Box::new(InvertEvaluator),
    )
}

pub struct InvertEvaluator;

impl BrushNodeEvaluator for InvertEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![(
            "output".into(),
            ScalarValue::Scalar(1.0 - ctx.input_f32("input")),
        )]
    }

    /// The same one subtraction as the CPU path, so the two stay in parity by
    /// construction. `cctx.input` yields either the wired (already remapped)
    /// expression or a literal, and the format string serves both.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if cctx.consumed_outputs.contains("output") {
            let input = cctx.input("input").as_f32();
            wgsl.outputs
                .insert("output".into(), format!("(1.0 - ({input}))"));
        }
        Ok(wgsl)
    }
}
