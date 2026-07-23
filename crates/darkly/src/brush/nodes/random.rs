//! Random node — per-dab or per-stroke random value.
//!
//! Outputs a single scalar random value in `[0, 1)` — the raw PRNG natural
//! range. Declares this as its wire-side `natural_range` so the runner
//! remaps to whichever range the downstream port wants (e.g. `[0, 1024]`
//! for `shape.seed`, `[-TAU, TAU]` for `shape.rotation`). A consumer that
//! wants bipolar values just declares a `[-x, x]` natural range on its
//! input — no special casing in this node.
//!
//! The mode param selects per-dab (changes every dab) or per-stroke
//! (constant within a stroke). Multiple instances in the same graph
//! produce independent sequences — the node's own ID salts the PRNG
//! seed automatically.

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, DabField, NodeWgsl, UniformField, WgslType};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "random";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "input",
            display_name: "Random",
            description: "A new random value for every brush mark — useful for jittering size, color, angle, etc.",
            ports: vec![
                // Compile-time branch selector — 0 = per-dab, 1 = per-stroke.
                // A labeled dropdown; not wirable (an enum can't be driven
                // per-dab), exposable like any input.
                PortDef::input("mode", BrushWireType::Enum)
                    .with_enum_options(["Per-Dab", "Per-Stroke"])
                    .with_value(InputValue::Int(0))
                    .with_label("Mode")
                    .with_description(
                        "Per-Dab changes the value every brush mark; Per-Stroke holds it constant for the stroke.",
                    ),
                PortDef::output("value", BrushWireType::Scalar)
                    .with_natural_range(0.0, 1.0)
                    .with_description("Random value in [0, 1)"),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_fallback_icon: None,
        },
        || Box::new(RandomEvaluator),
    )
}

pub struct RandomEvaluator;

impl BrushNodeEvaluator for RandomEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let mode = ctx.input("mode").as_f32() as i32;

        let value = match mode {
            1 => ctx.prng_at(0),             // per-stroke: constant
            _ => ctx.prng_at(ctx.dab_index), // per-dab: varies
        };

        vec![("value".into(), ScalarValue::Scalar(value))]
    }

    /// `random` is CPU-evaluated. Its output is the hashed scalar
    /// already computed by `evaluate_cpu` and stored in the slot
    /// table. The terminal extracts it and packs into either the
    /// dab record (per-dab mode) or the uniform buffer (per-stroke
    /// mode). The WGSL just reads the value back as a constant.
    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("value") {
            return Ok(wgsl);
        }
        let mode = cctx.input("mode").enum_index();
        let per_stroke = mode == 1;

        if per_stroke {
            let field_name = cctx.uniform_field_name("value");
            let key = field_name.clone();
            wgsl.uniform_fields.push(UniformField {
                name: field_name.clone(),
                ty: WgslType::F32,
                pack: Arc::new(move |outputs, bytes| {
                    let v = outputs.get(&key).map(|s| s.as_f32()).unwrap_or(0.0);
                    bytes.extend_from_slice(bytemuck::bytes_of(&v));
                }),
            });
            wgsl.outputs
                .insert("value".into(), format!("u.{}", field_name));
        } else {
            let field_name = cctx.dab_field_name("value");
            let key = field_name.clone();
            wgsl.dab_fields.push(DabField {
                name: field_name.clone(),
                ty: WgslType::F32,
                pack: Arc::new(move |outputs, bytes| {
                    let v = outputs.get(&key).map(|s| s.as_f32()).unwrap_or(0.0);
                    bytes.extend_from_slice(bytemuck::bytes_of(&v));
                }),
            });
            wgsl.outputs
                .insert("value".into(), format!("d.{}", field_name));
        }
        Ok(wgsl)
    }
}
