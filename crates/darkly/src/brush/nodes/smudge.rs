//! Smudge terminal: per-dab fragment-pass smear with a per-brush
//! compiled WGSL shader.
//!
//! Rides the shared [read-mirror terminal](crate::brush::read_mirror_terminal)
//! infrastructure (the per-brush pipeline, dab-meta queue, flush loop,
//! and `copy_origin` plumbing it shares with `liquify` and `blur`). This
//! file owns only what's smudge-specific: the read half-extent and the
//! variant WGSL.
//!
//! Each smudge dab samples the scratch read mirror twice: once at
//! `target_pos` (current background) and once at `target_pos − motion`
//! (the smear sample, what was under the brush at the previous dab). It
//! then mixes the two by `rate × mask × selection × stroke_opacity`. Per-dab
//! serialization is *semantically required*: each dab must see the prior
//! dab's output, which a single instanced draw can't express.
//!
//! Stationary dabs (`|motion| < 0.5 px`) are dropped before queueing:
//! `mix(bg, src, _)` collapses to identity in that regime. The read
//! region is expanded by `|motion|` per axis (ceiled for the bilinear
//! sampler's half-texel reach) so the smear sample at `target_pos −
//! motion` always lies inside the mirror snapshot.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::read_mirror_terminal::{
    self as rmt, read_mirror_pipeline_reg, ReadMirrorTerminal,
};
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::preview::{PreviewBackdrop, PreviewStaging};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

/// Motion magnitude (canvas pixels) below which the dab is treated as
/// stationary and dropped before queueing: `mix(bg, src, _)` is an
/// identity write when `src == bg`.
const STATIONARY_THRESHOLD_PX: f32 = 0.5;

pub const TYPE_ID: &str = "smudge";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration {
        pipelines: vec![read_mirror_pipeline_reg("smudge")],
        evaluator: || Box::new(SmudgeEvaluator),
        lifecycle: crate::brush::node::Lifecycle::SeedScratchFromPreStroke,
        scratch_format: crate::brush::node::COLOR_SCRATCH_FORMAT,
        node: NodeRegistration {
            type_id: TYPE_ID,
            category: "output",
            display_name: "Smudge",
            description: "Output that drags existing canvas pixels along the stroke, like smearing wet paint with a finger.",
            ports: vec![
                PortDef::input("position", BrushWireType::Vec2)
                    .with_description("Canvas-pixel pen tip for this dab"),
                PortDef::input("motion", BrushWireType::Vec2)
                    .with_description("Per-dab motion vector: the offset to sample from"),
                PortDef::input("size", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Size")
                    .with_unit(UnitType::Percent)
                    .with_description(
                        "Per-touch size multiplier (wire pressure here for pressure-sensitive size). Multiplies onto the brush's base size, owned by pen_input.",
                    ),
                PortDef::input("rate", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 0.6)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Smudge")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:paint-roller")
                    .exposed()
                    .with_description(
                        "How strongly each touch drags the canvas along the stroke. \
                         Higher values produce a longer smear trail; lower values \
                         barely move pixels.",
                    ),
                PortDef::input("opacity", BrushWireType::Scalar)
                    .with_range(0.0, 1.0, 1.0)
                    .with_natural_range(0.0, 1.0)
                    .with_label("Opacity")
                    .with_unit(UnitType::Percent)
                    .with_icon("fa6-solid:droplet")
                    .exposed()
                    .with_description(
                        "Overall stroke strength. Lower values reduce how much the smudge affects the canvas.",
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
                icon: "mdi:gesture-swipe",
                backdrop: PreviewBackdrop::Stripes,
            }),
        },
    }
}

pub struct SmudgeEvaluator;

impl ReadMirrorTerminal for SmudgeEvaluator {
    const PIPELINE_ID: &'static str = "smudge";
    const LABEL: &'static str = "smudge";

    fn read_half(&self, ctx: &EvalContext, _radius: f32, bbox_radius: f32) -> Option<[f32; 2]> {
        let motion = ctx.input("motion").as_vec2();
        // Stationary-dab early-out: `mix(bg, src, _)` is identity in this
        // regime. Skipping the queue saves a render pass and a mirror copy.
        if motion[0].abs() < STATIONARY_THRESHOLD_PX && motion[1].abs() < STATIONARY_THRESHOLD_PX {
            return None;
        }
        // Expand the read region by `|motion|` per axis so the smear
        // sample at `target_pos − motion` always lies inside the mirror
        // snapshot. Ceil to cover the bilinear sampler's half-texel reach.
        Some([
            bbox_radius + motion[0].abs().ceil(),
            bbox_radius + motion[1].abs().ceil(),
        ])
    }

    fn compile_body(
        &self,
        cctx: &CompileWgslCtx,
        copy_origin_field: &str,
    ) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();

        let mask_expr = cctx.input("mask").as_f32();
        let motion_expr = cctx.input("motion").as_vec2();
        let rate_expr = cctx.input("rate").as_f32();
        let opacity_expr = cctx.input("opacity").as_f32();

        wgsl.body = format!(
            "    let mask = clamp({mask_expr}, 0.0, 1.0);\n\
             \x20   let motion_v = {motion_expr};\n\
             \x20   let rate = clamp({rate_expr}, 0.0, 1.0);\n\
             \x20   let stroke_opacity = clamp({opacity_expr}, 0.0, 1.0);\n\
             \x20   let mirror_dims = vec2<f32>(textureDimensions(scratch_mirror_tex));\n\
             \x20   let bg_uv = (target_pos - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let src_uv = (target_pos - motion_v - d.{copy_origin_field}) / mirror_dims;\n\
             \x20   let bg = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, bg_uv, 0.0);\n\
             \x20   let src = textureSampleLevel(scratch_mirror_tex, scratch_mirror_smp, src_uv, 0.0);\n\
             \x20   let amount = clamp(rate * mask * sel * stroke_opacity, 0.0, 1.0);\n\
             \x20   return mix(bg, src, amount);\n",
        );

        Ok(wgsl)
    }

    /// Preview body, showing the footprint, not the smear: neutral gray
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

impl BrushNodeEvaluator for SmudgeEvaluator {
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
