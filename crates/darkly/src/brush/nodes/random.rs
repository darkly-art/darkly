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
        category: "sensor",
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

/// Deterministic PRNG: hash seed + index to produce a 0-1 float.
/// Uses a simple xorshift-style hash for speed.
fn prng_f32(seed: u32, index: u32) -> f32 {
    let mut h = seed.wrapping_add(index.wrapping_mul(2654435761));
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    (h & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

pub struct RandomEvaluator;

impl BrushNodeEvaluator for RandomEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let mode = match ctx.params.first() {
            Some(crate::gpu::params::ParamValue::Int(v)) => *v,
            _ => 0,
        };

        // Salt the stroke seed with this node's ID so multiple random
        // nodes in the same graph produce independent sequences.
        let salt = ctx.node_id.0 as u32;
        let salted_seed = ctx.stroke_seed.wrapping_add(salt.wrapping_mul(0x9E3779B9));

        let raw = match mode {
            1 => prng_f32(salted_seed, 0),             // per-stroke: constant
            _ => prng_f32(salted_seed, ctx.dab_index), // per-dab: varies
        };

        // Map 0..1 → -1..1
        let value = raw * 2.0 - 1.0;

        vec![("value".into(), ScalarValue::Scalar(value))]
    }
}
