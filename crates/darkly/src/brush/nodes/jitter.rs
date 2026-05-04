//! Jitter node — `input + uniform(-amount, +amount)`, fresh per dab.
//!
//! Same units as whatever you wire into `amount`: radians when driving
//! rotation, port-space fractions for size, etc. Multiple instances in
//! the same graph produce independent streams via the node-ID salt
//! baked into `EvalContext::prng_at`.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "jitter",
        category: "modulate",
        display_name: "Jitter",
        ports: vec![
            PortDef::input("input", BrushWireType::Scalar)
                .with_description("Base signal to perturb"),
            PortDef::input("amount", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_label("Amount")
                .with_icon("fa-solid fa-shuffle")
                .with_description(
                    "Magnitude of the added noise, in the same units as the \
                     target port. Output is input ± amount, uniform.",
                ),
            PortDef::output("value", BrushWireType::Scalar)
                .with_description("input + uniform(-amount, +amount)"),
        ],
        params: &[],
        is_gpu: false,
    }
}

pub struct JitterEvaluator;

impl BrushNodeEvaluator for JitterEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let input = ctx.input_f32("input");
        let amount = ctx.input_f32("amount");
        let raw = ctx.prng_at(ctx.dab_index); // 0..1
        let noise = (raw * 2.0 - 1.0) * amount; // -amount..+amount
        vec![("value".into(), ScalarValue::Scalar(input + noise))]
    }
}
