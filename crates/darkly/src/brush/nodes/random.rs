//! Random node — per-dab or per-stroke random value.
//!
//! Outputs a single scalar random value in `[0, 1)` — the raw PRNG natural
//! range. Declares this as its wire-side `natural_range` so the runner
//! remaps to whichever range the downstream port wants (e.g. `[0, 1024]`
//! for `circle.seed`, `[-TAU, TAU]` for `circle.phase`). A consumer that
//! wants bipolar values just declares a `[-x, x]` natural range on its
//! input — no special casing in this node.
//!
//! The mode param selects per-dab (changes every dab) or per-stroke
//! (constant within a stroke). Multiple instances in the same graph
//! produce independent sequences — the node's own ID salts the PRNG
//! seed automatically.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(NodeRegistration {
        type_id: "random",
        category: "input",
        display_name: "Random",
        ports: vec![PortDef::output("value", BrushWireType::Scalar)
            .with_natural_range(0.0, 1.0)
            .with_description("Random value in [0, 1)")],
        params: &[
            // Enum stored as Int — 0 = per-dab, 1 = per-stroke. Surfaced
            // as a labeled dropdown so users don't have to memorize
            // indices; the evaluator's match arms read the same i32.
            ParamDef::Enum {
                name: "mode",
                options: &["Per-Dab", "Per-Stroke"],
                default: 0,
            },
        ],
        is_gpu: false,
    })
}

pub struct RandomEvaluator;

impl BrushNodeEvaluator for RandomEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let mode = match ctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => *v,
            _ => 0,
        };

        let value = match mode {
            1 => ctx.prng_at(0),             // per-stroke: constant
            _ => ctx.prng_at(ctx.dab_index), // per-dab: varies
        };

        vec![("value".into(), ScalarValue::Scalar(value))]
    }
}
