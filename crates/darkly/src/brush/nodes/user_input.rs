//! User Input node: a labelled dial the brush author places in the graph and
//! wires to as many inputs as they like.
//!
//! The graph could already fan one control out to several sinks by exposing a
//! math node's input and wiring its output around (place a `multiply`, leave
//! `b` at its identity, expose `a`), but that costs a node whose arithmetic is
//! a no-op and whose name says "Multiply" when it means "knob". This node says
//! it directly.
//!
//! It carries no params. Everything an author would want to author about the
//! dial (its label, description, icon, slider range, inversion) is already
//! per-instance state on the brush-bar entry, reachable from the entry modal,
//! so putting any of it here would store the same fact twice.
//!
//! The whole node is one settable-source input. `PortDef::source` lets an
//! input keep its scrubbable authored value while other nodes wire *from* it,
//! and the rest of the stack is generic over that: the compiler gives it an
//! output slot, `execute_cpu` republishes its resolved value onto that slot,
//! and the brush bar disqualifies an entry only for *incoming* wires. So a
//! dial with ten outgoing wires is still a dial.
//!
//! `persist_in_thumbnail` is load-bearing. `reset_exposed_scrubs` neutralises
//! exposed ports back to their registration defaults before baking a brush's
//! picker thumbnail, so the icon shows brush identity rather than a momentary
//! scrub. For a generic dial that would be backwards: the registration default
//! is an arbitrary mid-travel number and the author's value *is* the identity.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::BrushWireType;
use crate::brush::wire::ScalarValue;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub const TYPE_ID: &str = "user_input";

/// Mid-travel. A dial is as likely to feed an additive or selector input as a
/// gain, so there is no identity value to prefer.
const DEFAULT_VALUE: f32 = 0.5;

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "input",
            display_name: "User Input",
            description: "A dial the painter turns from the brush bar. Wire it \
                          into as many inputs as you like and they all follow it.",
            ports: vec![
                // `UnitType::Raw` (identity, no suffix) rather than `Percent`:
                // a generic dial has no unit, and once the author re-ranges it
                // for a pixel or angle sink a percentage reads as nonsense
                // (0..64 would display as 0% to 6400%). Raw is never wrong,
                // only plain.
                //
                // No `natural_range` for the same reason: `apply_wire_remap`
                // rescales only when both ends declare one, and a dial that
                // silently rescaled itself differently per sink would defeat
                // the point of one control driving many.
                PortDef::input("value", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, DEFAULT_VALUE)
                    .with_unit(UnitType::Raw)
                    .with_icon("fa6-solid:sliders")
                    .with_label("Value")
                    .exposed()
                    .source()
                    .persist_in_thumbnail()
                    .with_description(
                        "The value the painter sets. Wire it into as many \
                         inputs as you like; they all follow this one control.",
                    ),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
        },
        || Box::new(UserInputEvaluator),
    )
}

pub struct UserInputEvaluator;

impl BrushNodeEvaluator for UserInputEvaluator {
    /// Nothing to compute. `execute_cpu` republishes every settable-source
    /// input's resolved value onto its own output slot, which is the whole of
    /// this node's behaviour on the CPU.
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    /// Publish the authored value as the expression each consuming use
    /// substitutes. The port is only ever undriven here: `Graph::connect`
    /// retires the wires leaving a settable-source the moment that source is
    /// driven, so a driven dial has no consumers to compile for.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if cctx.consumed_outputs.contains("value") {
            wgsl.outputs
                .insert("value".into(), cctx.input("value").as_f32());
        }
        Ok(wgsl)
    }
}
