//! Stamp dab generation GPU node.
//!
//! Image-based dab source — receives a brush tip texture handle on its
//! `tip` input (typically from an Image node) and stamps it onto a dab
//! texture with size, rotation, mirror, ratio, opacity, and color
//! transforms via the `stamp.wgsl` shader.
//!
//! If the `tip` input is disconnected (no upstream image), the node
//! produces no output — no tip means no dab.
//!
//! The dab viewport may be non-square: if the tip texture has a non-square
//! aspect ratio, the viewport preserves it so the tip is sampled without
//! distortion.  The `size` input (0-1) scales the longer axis up to
//! `MAX_DAB_SIZE`; the shorter axis follows from the tip aspect ratio.

use crate::brush::brush_tip::BrushTipApplication;
use crate::brush::dab_pool::MAX_DAB_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::pipelines::StampUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue, TextureHandle};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "stamp",
        category: "gpu",
        display_name: "Stamp Tip",
        ports: vec![
            PortDef::input("tip", BrushWireType::Texture)
                .with_description("Brush tip image"),
            PortDef::input("size", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5)
                .with_label("Size")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-circle")
                .with_description("Base brush size"),
            PortDef::input("scale", BrushWireType::Scalar)
                .with_range(0.0, 4.0, 0.1)
                .with_label("Scale")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-up-right-and-down-left-from-center")
                .exposed()
                .with_description("Size multiplier"),
            PortDef::input("rotation", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_label("Rotation")
                .with_unit(UnitType::Degrees)
                .with_icon("fa-solid fa-rotate")
                .with_description("Brush rotation angle"),
            PortDef::input("mirror_x", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_description("Flip horizontally"),
            PortDef::input("mirror_y", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_description("Flip vertically"),
            PortDef::input("ratio", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0)
                .with_label("Ratio")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-arrows-left-right")
                .with_description("Aspect ratio (100% = round)"),
            PortDef::input("flow", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0)
                .with_label("Flow")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-droplet")
                .exposed()
                .with_description("Paint deposited per dab"),
            PortDef::input("color", BrushWireType::Color)
                .with_description("Brush color"),
            PortDef::output("dab", BrushWireType::Texture)
                .with_description("The stamped dab texture ready for compositing"),
            PortDef::output("dab_size", BrushWireType::Vec2)
                .with_description("Actual pixel dimensions of the generated dab"),
            PortDef::output("preview", BrushWireType::Texture)
                .with_description("Hover-preview texture: brush tip with rotation/ratio/mirror baked in, deposition (flow/color) neutralised. Texture dimensions encode the brush's canvas-pixel extent."),
        ],
        params: &[
            ParamDef::Int { name: "application", min: 0, max: 3, default: 0 },
        ],
        is_gpu: true,
    }
}

/// Fully-resolved stamp inputs, computed once and reused by both the live
/// evaluation path and the preview path. Everything here is pure CPU data.
struct StampInputs {
    tip_handle: TextureHandle,
    effective_size: f32,      // size * scale, clamped to [0, 1]
    ratio: f32,
    /// Per-dab paint deposition (industry "flow"). Feeds the stamp shader's
    /// `opacity` uniform — the stamp pipeline still calls its uniform
    /// `opacity` because it represents the per-dab alpha. Stroke-level
    /// opacity is applied later in the commit pass.
    flow: f32,
    color: [f32; 4],
    rotation_rad: f32,
    mirror_x: f32,
    mirror_y: f32,
    application_int: u32,
}

fn resolve_inputs(ctx: &EvalContext) -> Option<StampInputs> {
    let tip_handle = match ctx.input("tip") {
        ScalarValue::Texture(h) => h,
        _ => return None,
    };

    let size = ctx.input_f32("size");
    let scale = ctx.input_f32("scale");
    let rotation_input = ctx.input_f32("rotation");
    let mirror_x_input = ctx.input_f32("mirror_x");
    let mirror_y_input = ctx.input_f32("mirror_y");
    let ratio = ctx.input_f32("ratio").max(0.01);
    let flow = ctx.input_f32("flow");
    let color = ctx.input("color").as_color();

    let application_int = match ctx.params.get(0) {
        Some(crate::gpu::params::ParamValue::Int(v)) => *v as u32,
        _ => 0,
    };
    let _application = match application_int {
        1 => BrushTipApplication::ImageStamp,
        2 => BrushTipApplication::LightnessMap,
        3 => BrushTipApplication::GradientMap,
        _ => BrushTipApplication::AlphaMask,
    };

    Some(StampInputs {
        tip_handle,
        effective_size: (size * scale).clamp(0.0, 1.0),
        ratio,
        flow,
        color,
        rotation_rad: rotation_input * std::f32::consts::TAU,
        mirror_x: if mirror_x_input > 0.5 { 1.0 } else { 0.0 },
        mirror_y: if mirror_y_input > 0.5 { 1.0 } else { 0.0 },
        application_int,
    })
}

/// Compute the dab's pixel dimensions given the effective size and the tip's
/// aspect ratio. The longer axis scales up to `max_dim`, the shorter axis
/// follows from the tip's aspect. Both are clamped into [1, max_dim].
fn compute_dab_dims(effective_size: f32, tip_w: u32, tip_h: u32, max_dim: u32) -> (u32, u32) {
    let max = max_dim as f32;
    let tip_aspect = tip_w as f32 / tip_h as f32;
    if tip_aspect >= 1.0 {
        let w = (effective_size * max).max(1.0);
        let h = (w / tip_aspect).max(1.0);
        (w.ceil().min(max) as u32, h.ceil().min(max) as u32)
    } else {
        let h = (effective_size * max).max(1.0);
        let w = (h * tip_aspect).max(1.0);
        (w.ceil().min(max) as u32, h.ceil().min(max) as u32)
    }
}

/// Record a single stamp render pass into `target_view` at the given pixel
/// viewport size. Shared by live stroke evaluation (target = pool dab) and
/// preview (target = overlay's preview mask).
///
/// Split-borrow friendly: takes the pieces it needs rather than `&mut gpu`,
/// so the caller can hold a `gpu.dab_pool.view(target_handle)` borrow
/// concurrently without a conflict.
fn encode_stamp_pass(
    encoder: &mut wgpu::CommandEncoder,
    queue: &wgpu::Queue,
    pipelines: &crate::brush::pipelines::BrushPipelines,
    tip_bind_group: &wgpu::BindGroup,
    inputs: &StampInputs,
    target_view: &wgpu::TextureView,
    viewport: (u32, u32),
    label: &'static str,
) {
    let (view_w, view_h) = viewport;

    let uniforms = StampUniforms {
        dab_width: view_w as f32,
        dab_height: view_h as f32,
        opacity: inputs.flow,
        rotation: inputs.rotation_rad,
        color: inputs.color,
        mirror_x: inputs.mirror_x,
        mirror_y: inputs.mirror_y,
        application: inputs.application_int,
        ratio: inputs.ratio,
    };
    let offset = pipelines.write_stamp_uniforms(queue, &uniforms);

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_viewport(0.0, 0.0, view_w as f32, view_h as f32, 0.0, 1.0);
    pass.set_pipeline(pipelines.stamp_pipeline());
    pass.set_bind_group(0, &pipelines.stamp_uniform_bind_group, &[offset]);
    pass.set_bind_group(1, tip_bind_group, &[]);
    pass.draw(0..3, 0..1);
}

pub struct StampEvaluator;

impl BrushNodeEvaluator for StampEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        // GPU node — CPU evaluation is a no-op.
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(inputs) = resolve_inputs(ctx) else { return vec![]; };

        let (tip_w, tip_h) = gpu.dab_pool.texture_size(inputs.tip_handle);
        let (dab_w, dab_h) = compute_dab_dims(inputs.effective_size, tip_w, tip_h, MAX_DAB_SIZE);

        let handle = gpu.dab_pool.acquire(gpu.device);
        let dab_view = gpu.dab_pool.view(handle).clone();
        let tip_bind_group = gpu.dab_pool.bind_group(inputs.tip_handle).clone();
        encode_stamp_pass(
            &mut gpu.encoder, gpu.queue, gpu.pipelines,
            &tip_bind_group, &inputs, &dab_view, (dab_w, dab_h), "brush-stamp",
        );

        vec![
            ("dab".into(), ScalarValue::Texture(handle)),
            ("dab_size".into(), ScalarValue::Vec2([dab_w as f32, dab_h as f32])),
        ]
    }

    /// Preview-mode render: produce a brush-tip texture with rotation,
    /// ratio, and mirror baked in — but *without* per-dab deposition
    /// modulation (flow=1, color=white). The texture is sized to the
    /// brush's canvas-pixel footprint, so downstream consumers (typically
    /// `color_output`) can read its dimensions directly without a separate
    /// size wire.
    ///
    /// The same `encode_stamp_pass` shader as the stroke path is used —
    /// single source of truth for tip rasterisation. The only difference
    /// is the input values fed in.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let Some(mut inputs) = resolve_inputs(ctx) else { return vec![]; };

        // Strip deposition modulation. The preview shows the brush, not
        // how much paint a single dab would deposit.
        inputs.flow = 1.0;
        inputs.color = [1.0, 1.0, 1.0, 1.0];

        let (tip_w, tip_h) = gpu.dab_pool.texture_size(inputs.tip_handle);
        let (dab_w, dab_h) = compute_dab_dims(inputs.effective_size, tip_w, tip_h, MAX_DAB_SIZE);
        if dab_w == 0 || dab_h == 0 {
            return vec![];
        }

        // Right-sized texture: dimensions == brush canvas-pixel extent.
        // `color_output::render_preview` queries `texture_size(handle)` to
        // recover this without a parallel size wire.
        let handle = gpu.dab_pool.acquire_sized(gpu.device, dab_w, dab_h);
        let view = gpu.dab_pool.view(handle).clone();
        let tip_bind_group = gpu.dab_pool.bind_group(inputs.tip_handle).clone();
        encode_stamp_pass(
            &mut gpu.encoder, gpu.queue, gpu.pipelines,
            &tip_bind_group, &inputs, &view, (dab_w, dab_h), "brush-stamp-preview",
        );

        vec![
            ("preview".into(), ScalarValue::Texture(handle)),
            ("dab_size".into(), ScalarValue::Vec2([dab_w as f32, dab_h as f32])),
        ]
    }
}
