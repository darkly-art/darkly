//! Blur terminal — per-dab fragment-pass neighborhood average with a
//! per-brush compiled WGSL shader.
//!
//! Rides the shared [read-mirror terminal](crate::brush::read_mirror_terminal)
//! infrastructure (the per-brush pipeline, dab-meta queue, flush loop,
//! and `copy_origin` plumbing it shares with `smudge` and `liquify`).
//! This file owns only what's blur-specific: the read half-extent, the
//! per-dab kernel size, and the variant WGSL.
//!
//! Each blur dab samples a golden-angle disc of the scratch read mirror
//! around `target_pos`, averages the taps, and mixes the result over the
//! original by `mask × selection × opacity`. The disc radius is
//! `blur_px = strength × radius`.
//!
//! ## Dwell-compounding (intentional — do not "optimize" away)
//!
//! A neighborhood average has no inter-dab data dependency, so blur does
//! **not** *need* per-dab serialization for correctness. It
//! rides the per-dab serialized path *deliberately*: each dab reads the
//! *cumulative* scratch (the prior dab's writeback, seen through the
//! mirror), so overlapping dabs and scrubbing back-and-forth
//! progressively re-blur already-blurred pixels — like Photoshop's Blur
//! tool and Krita's blur paintop. Collapsing the flush into a single
//! instanced pass would silently remove that dwell-compounding behavior.
//!
//! ## Size-variant (intentional — the opposite of liquify)
//!
//! `blur_px = strength × radius`, so a bigger brush blurs *more*. This is
//! the deliberate opposite of [`liquify`](super::liquify)'s size-invariant
//! push, where the size slider scales only the warped extent, not the
//! intensity.

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::read_mirror_terminal::{
    self as rmt, insert_slot_output, read_mirror_pipeline_reg, ReadMirrorTerminal,
};
use crate::brush::wgsl::{CompileWgslCtx, DabField, NodeWgsl, WgslType};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::preview::{PreviewBackdrop, PreviewStaging};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

/// Per-dab strength below which the dab is dropped — `mix(orig, blurred, 0)`
/// is an identity write.
const STRENGTH_EPSILON: f32 = 1.0e-4;

/// Brush radius below which the dab is dropped — a sub-pixel disc has no
/// neighborhood to average.
const MIN_RADIUS_PX: f32 = 1.0;

/// Number of golden-angle disc taps the kernel averages (plus the centre
/// tap). 24 fills the disc densely enough to read as a smooth blur
/// without making each per-dab fragment pass expensive.
const BLUR_TAPS: u32 = 24;

/// Kernel radius at full strength, as a fraction of the brush radius.
/// `blur_px = strength × radius × MAX_KERNEL_FRACTION`, so the slider's
/// full 0–100% travel maps onto a useful band — past ~25% of the radius
/// each touch reaches so far it smears rather than softens.
const MAX_KERNEL_FRACTION: f32 = 0.25;

pub const TYPE_ID: &str = "blur";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![read_mirror_pipeline_reg("blur")],
        evaluator: || Box::new(BlurEvaluator),
        lifecycle: crate::brush::node::Lifecycle::SeedScratchFromPreStroke,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Blur",
            description: "Output that softens existing canvas pixels under the stroke by averaging a neighborhood, compounding where the brush dwells.",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Canvas-pixel pen tip for this dab"),
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_description(
                        "Per-touch size multiplier (wire pressure here for pressure-sensitive size). Multiplies onto the brush's base size, owned by pen_input.",
                    ),
                PortDef::input("strength", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.05)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Strength")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:gauge-high")
                    .exposed()
                    .with_description(
                        "How wide a neighborhood each touch averages, as a fraction of the brush \
                         radius. Higher values soften more per touch.",
                    ),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:droplet")
                    .exposed()
                    .with_description(
                        "Overall stroke strength. Lower values blend less of the blurred result \
                         over the original.",
                    ),
                PortDef::input("mask", BrushWireType::Scalar).with_description(
                    "Per-fragment shape mask (typically wired from circle.mask)",
                ),
                PortDef::output("dab_size", BrushWireType::Vec2)
                    .with_description("Brush mark size in canvas pixels"),
            ],
            is_gpu: true,
            is_terminal: true,
            supports_erase: false,
            preview_staging: Some(PreviewStaging {
                icon: "mdi:blur",
                backdrop: PreviewBackdrop::Stripes,
            }),
        },
    }
}

pub struct BlurEvaluator;

impl ReadMirrorTerminal for BlurEvaluator {
    const PIPELINE_ID: &'static str = "blur";
    const LABEL: &'static str = "blur";

    fn read_half(&self, ctx: &EvalContext, radius: f32, bbox_radius: f32) -> Option<[f32; 2]> {
        let strength = ctx.input_f32("strength").clamp(0.0, 1.0);
        if strength < STRENGTH_EPSILON || radius < MIN_RADIUS_PX {
            return None;
        }
        // Kernel disc radius. The `+ 1.0` covers the bilinear sampler's
        // half-texel reach past the max tap offset; the shared
        // `read_half.max(write_half)` clamp then handles a tiny `blur_px`
        // where `bbox_radius` dominates.
        let blur_px = strength * radius * MAX_KERNEL_FRACTION;
        let half = bbox_radius + blur_px + 1.0;
        Some([half, half])
    }

    fn pack_extra(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext, node_id: &str, radius: f32) {
        // Per-dab kernel radius, so a pressure-wired strength rides the
        // dab record. Inserted before `queue_dab` packs the record.
        let strength = ctx.input_f32("strength").clamp(0.0, 1.0);
        let blur_px = strength * radius * MAX_KERNEL_FRACTION;
        insert_slot_output(gpu, node_id, "blur_px", ScalarValue::Scalar(blur_px));
    }

    fn compile_body(
        &self,
        cctx: &CompileWgslCtx,
        copy_origin_field: &str,
    ) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();

        let mask_expr = cctx.input("mask").as_f32();
        let opacity_expr = cctx.input("opacity").as_f32();

        // Per-dab kernel radius (canvas px). `evaluate_gpu`'s `pack_extra`
        // inserts the value into `dab_batch.slot_outputs`; the packer
        // reads it through the standard `pack_dab_record` path.
        let blur_field = cctx.dab_field_name("blur_px");
        let key = blur_field.clone();
        wgsl.dab_fields.push(DabField {
            name: blur_field.clone(),
            ty: WgslType::F32,
            pack: Arc::new(move |outputs, bytes| {
                let v = outputs.get(&key).map(|s| s.as_f32()).unwrap_or(0.0);
                bytes.extend_from_slice(bytemuck::bytes_of(&v));
            }),
        });

        // Golden-angle spiral disc of taps over the scratch read mirror,
        // averaged into `blurred`. Each tap's UV is clamped inside the
        // mirror bounds so high-strength edge dabs never sample undefined
        // texels.
        //
        // Golden-angle bokeh / sunflower-disc sampling technique adapted
        // from the `lens_blur` veil — Shadertoy bokeh by Dave Hoskins
        // et al., https://www.shadertoy.com/playlist/fXlGDN
        let taps = BLUR_TAPS;
        let taps_f = format!("{:.1}", taps as f32);
        wgsl.body = format!(
            "    let mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   let stroke_opacity = clamp({opacity_expr}, 0.0, 1.0);\n\
             \x20   let amount = clamp(mask * sel * stroke_opacity, 0.0, 1.0);\n\
             \x20   let mirror_dims = vec2<f32>(textureDimensions(scratch_mirror_tex));\n\
             \x20   let centre_local = target_pos - d.{copy_origin_field};\n\
             \x20   let centre_uv = clamp(centre_local, vec2<f32>(0.5), mirror_dims - vec2<f32>(0.5)) / mirror_dims;\n\
             \x20   let original = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, centre_uv, 0.0);\n\
             \x20   if (amount <= 0.0) {{ return original; }}\n\
             \x20   let blur_px = d.{blur_field};\n\
             \x20   // Golden angle ≈ 137.508°: cos ≈ -0.7374, sin ≈ 0.6755.\n\
             \x20   let ga_cos = -0.7374;\n\
             \x20   let ga_sin = 0.6755;\n\
             \x20   var p = vec2<f32>(1.0, 0.0);\n\
             \x20   var acc = original;\n\
             \x20   var count = 1.0;\n\
             \x20   for (var i = 0u; i < {taps}u; i = i + 1u) {{\n\
             \x20       p = vec2<f32>(p.x * ga_cos - p.y * ga_sin, p.x * ga_sin + p.y * ga_cos);\n\
             \x20       // sqrt distributes taps evenly over the disc area.\n\
             \x20       let r = blur_px * sqrt((f32(i) + 1.0) / {taps_f});\n\
             \x20       let sample_local = centre_local + p * r;\n\
             \x20       let uv = clamp(sample_local, vec2<f32>(0.5), mirror_dims - vec2<f32>(0.5)) / mirror_dims;\n\
             \x20       acc = acc + textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, uv, 0.0);\n\
             \x20       count = count + 1.0;\n\
             \x20   }}\n\
             \x20   let blurred = acc / count;\n\
             \x20   return mix(original, blurred, amount);\n",
        );

        Ok(wgsl)
    }

    /// Preview body — show the footprint, not the blur: neutral gray
    /// modulated by the upstream shape mask so the cursor reads the
    /// brush's actual coverage area. (The stroke body's `scratch_mirror`
    /// bindings are omitted in preview mode.)
    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        let mask_expr = cctx.input("mask").as_f32();
        wgsl.body = format!(
            "    let mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   if (mask <= 0.0) {{ discard; }}\n\
             \x20   let preview_color = vec3<f32>(0.6, 0.6, 0.6);\n\
             \x20   return vec4<f32>(preview_color * mask, mask);\n"
        );
        Ok(wgsl)
    }
}

impl BrushNodeEvaluator for BlurEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        rmt::evaluate_gpu(self, ctx, gpu)
    }

    fn flush_dabs(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        rmt::flush_dabs::<Self>(gpu)
    }

    fn commit(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        rmt::commit(gpu)
    }

    fn render_cursor_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        rmt::render_cursor_preview(ctx, gpu)
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        rmt::compile_wgsl(self, cctx)
    }

    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        ReadMirrorTerminal::compile_cursor_preview_body(self, cctx)
    }
}
