//! Scatter node — jitter a position by a deterministic per-dab amount.
//!
//! Takes a `position` wire and emits a displaced position. The offset is
//! drawn from an internal per-axis PRNG in (-1, 1), scaled by the user-
//! controllable `amount_x` / `amount_y`, and then by `dab_size` (the
//! pixel reference the amounts are fractions of). Positioned in the
//! graph wherever jitter is wanted: before `color_output.position` for
//! deposition scatter, before a smudge sample point for texture-direction
//! noise, etc. Two independent scatter nodes in the same graph salt their
//! seeds with their own `node_id`, so they produce uncorrelated streams.
//!
//! The node is pure math — the compiler auto-promotes it to the GPU
//! phase when `dab_size` is wired from a GPU-produced output (e.g. via
//! `split_vec2` on `stamp.dab_size`).

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "scatter",
        category: "modulate",
        display_name: "Scatter",
        ports: vec![
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Position to displace"),
            PortDef::input("amount_x", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_natural_range(0.0, 1.0)
                .with_label("Amount X")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-arrows-left-right")
                .exposed()
                .with_description("Fraction of `dab_size` used as max horizontal displacement"),
            PortDef::input("amount_y", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_natural_range(0.0, 1.0)
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
                .with_description(
                    "Pixel reference the amounts are fractions of. \
                    Wire `stamp.dab_major` for size-proportional scatter, or leave \
                    unwired and dial it directly for smudge/liquify brushes.",
                ),
            PortDef::output("position", BrushWireType::Vec2)
                .with_description("Input position + random offset"),
        ],
        params: &[],
        is_gpu: false,
    }
}

pub struct ScatterEvaluator;

impl BrushNodeEvaluator for ScatterEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let position = ctx.input("position").as_vec2();
        let amount_x = ctx.input_f32("amount_x");
        let amount_y = ctx.input_f32("amount_y");
        let dab_size = ctx.input_f32("dab_size");

        // Two PRNG pulls per dab (one per axis) via `prng_at`'s index arg.
        let raw_x = ctx.prng_at(ctx.dab_index.wrapping_mul(2));
        let raw_y = ctx.prng_at(ctx.dab_index.wrapping_mul(2).wrapping_add(1));

        let offset_x = (raw_x * 2.0 - 1.0) * amount_x * dab_size;
        let offset_y = (raw_y * 2.0 - 1.0) * amount_y * dab_size;

        vec![(
            "position".into(),
            ScalarValue::Vec2([position[0] + offset_x, position[1] + offset_y]),
        )]
    }
}
