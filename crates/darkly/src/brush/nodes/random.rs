//! Random node — per-dab or per-stroke random value.
//!
//! Outputs a single scalar random value in [-1, 1].  The mode param
//! selects per-dab (changes every dab) or per-stroke (constant within
//! a stroke).  Multiple instances in the same graph produce independent
//! sequences — the node's own ID salts the PRNG seed automatically.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "random",
        category: "input",
        display_name: "Random",
        ports: vec![PortDef::output("value", BrushWireType::Scalar)
            .with_description("Random value (-1 to 1)")],
        params: &[
            // 0 = per-dab, 1 = per-stroke
            ParamDef::Int {
                name: "mode",
                min: 0,
                max: 1,
                default: 0,
            },
        ],
        is_gpu: false,
    }
}

pub struct RandomEvaluator;

impl BrushNodeEvaluator for RandomEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let mode = match ctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => *v,
            _ => 0,
        };

        let raw = match mode {
            1 => ctx.prng_at(0),             // per-stroke: constant
            _ => ctx.prng_at(ctx.dab_index), // per-dab: varies
        };

        // Map 0..1 → -1..1
        let value = raw * 2.0 - 1.0;

        vec![("value".into(), ScalarValue::Scalar(value))]
    }
}
