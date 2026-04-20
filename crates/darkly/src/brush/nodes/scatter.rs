//! Scatter node — jitter a position by a deterministic per-dab amount.
//!
//! Takes a `position` wire and emits a displaced position. The offset is
//! drawn from an internal per-axis PRNG in (-1, 1), scaled by the user-
//! controllable `amount_x` / `amount_y`, and optionally scaled again by
//! an axis-wise `dab_size` multiplier (when wired). Positioned in the
//! graph wherever jitter is wanted: before `color_output.position` for
//! deposition scatter, before a smudge sample point for texture-direction
//! noise, etc. Two independent scatter nodes in the same graph salt their
//! seeds with their own `node_id`, so they produce uncorrelated streams.
//!
//! Tagged `is_gpu: true` to sit in the GPU evaluation phase even though
//! it touches no GPU resources: the `dab_size` input is commonly wired
//! from a dab-producing node (e.g. `stamp`) whose dab_size isn't known
//! until after `execute_cpu`. Running scatter in the GPU phase lets
//! topo-order deliver that value.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "scatter",
        category: "math",
        display_name: "Scatter",
        ports: vec![
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Position to displace"),
            PortDef::input("amount_x", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_label("Amount X")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-arrows-left-right")
                .exposed()
                .with_description("Fraction of `dab_size` used as max horizontal displacement"),
            PortDef::input("amount_y", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_label("Amount Y")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-arrows-up-down")
                .exposed()
                .with_description("Fraction of `dab_size` used as max vertical displacement"),
            PortDef::input("dab_size", BrushWireType::Scalar)
                .with_range(0.0, 512.0, 100.0)
                .with_label("Dab Size")
                .with_unit(UnitType::Raw)
                .with_icon("fa-solid fa-ruler")
                .with_description("Pixel reference the amounts are fractions of. \
                    Wire `stamp.dab_major` for size-proportional scatter, or leave \
                    unwired and dial it directly for smudge/liquify brushes."),
            PortDef::output("position", BrushWireType::Vec2)
                .with_description("Input position + random offset"),
        ],
        params: &[],
        is_gpu: true,
    }
}

/// Deterministic PRNG: hash seed + index to produce a 0-1 float.
/// Same construction as the `random` node so replays and checkpoint
/// restores reproduce identical jitter.
fn prng_f32(seed: u32, index: u32) -> f32 {
    let mut h = seed.wrapping_add(index.wrapping_mul(2654435761));
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    (h & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

pub struct ScatterEvaluator;

impl BrushNodeEvaluator for ScatterEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        // Pure math, but scheduled in the GPU phase so topo-order can
        // deliver `dab_size` from a GPU node. See module doc.
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        _gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let position = ctx.input("position").as_vec2();
        let amount_x = ctx.input_f32("amount_x");
        let amount_y = ctx.input_f32("amount_y");
        let dab_size = ctx.input_f32("dab_size");

        // Independent streams per node via node_id salt; two PRNG pulls
        // per dab (one for each axis).
        let salt = ctx.node_id.0 as u32;
        let salted_seed = ctx.stroke_seed.wrapping_add(salt.wrapping_mul(0x9E3779B9));
        let raw_x = prng_f32(salted_seed, ctx.dab_index.wrapping_mul(2));
        let raw_y = prng_f32(salted_seed, ctx.dab_index.wrapping_mul(2).wrapping_add(1));

        let offset_x = (raw_x * 2.0 - 1.0) * amount_x * dab_size;
        let offset_y = (raw_y * 2.0 - 1.0) * amount_y * dab_size;

        vec![
            ("position".into(), ScalarValue::Vec2([
                position[0] + offset_x,
                position[1] + offset_y,
            ])),
        ]
    }
}
