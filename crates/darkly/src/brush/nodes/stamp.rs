//! Stamp dab generation GPU node.
//!
//! Image-based dab source — loads a brush tip texture cached in the
//! `DabTexturePool` and stamps it onto a dab texture with size, rotation,
//! mirror, ratio, opacity, and color transforms via the `stamp.wgsl` shader.
//!
//! This replaces `procedural.rs` as the dab source for image-based brushes.
//! The brush tip texture is uploaded once on brush load (by the preset
//! loading flow) and referenced by name through the `tip_name` parameter.
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
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "stamp",
        category: "gpu",
        display_name: "Stamp Tip",
        ports: vec![
            // Inputs (0-1 normalized, mapped to actual ranges internally).
            PortDef::input("size", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5),
            PortDef::input("rotation", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0),
            PortDef::input("mirror_x", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0),
            PortDef::input("mirror_y", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0),
            PortDef::input("ratio", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0),
            PortDef::input("opacity", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0),
            PortDef::input("color", BrushWireType::Color),
            PortDef::input("scatter_x", BrushWireType::Scalar)
                .with_range(-1.0, 1.0, 0.0),
            PortDef::input("scatter_y", BrushWireType::Scalar)
                .with_range(-1.0, 1.0, 0.0),
            // Outputs.
            PortDef::output("dab", BrushWireType::Texture),
            PortDef::output("dab_size", BrushWireType::Vec2),
            PortDef::output("scatter_offset", BrushWireType::Vec2),
        ],
        params: &[
            ParamDef::String { name: "tip_name", default: "" },
            ParamDef::Int { name: "application", min: 0, max: 3, default: 0 },
        ],
        is_gpu: true,
    }
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
        let size = ctx.input_f32("size");
        let rotation_input = ctx.input_f32("rotation");
        let mirror_x_input = ctx.input_f32("mirror_x");
        let mirror_y_input = ctx.input_f32("mirror_y");
        let ratio = ctx.input_f32("ratio").max(0.01);
        let opacity = ctx.input_f32("opacity");
        let color = ctx.input("color").as_color();
        let scatter_x = ctx.input_f32("scatter_x");
        let scatter_y = ctx.input_f32("scatter_y");

        // Read params.
        let tip_name = ctx.param_str(0);
        let application_int = match ctx.params.get(1) {
            Some(crate::gpu::params::ParamValue::Int(v)) => *v as u32,
            _ => 0,
        };
        let _application = match application_int {
            1 => BrushTipApplication::ImageStamp,
            2 => BrushTipApplication::LightnessMap,
            3 => BrushTipApplication::GradientMap,
            _ => BrushTipApplication::AlphaMask,
        };

        // Check that the tip texture is cached before proceeding.
        if !gpu.dab_pool.has_tip(tip_name) {
            log::warn!("stamp node: tip '{}' not found in cache", tip_name);
            return vec![];
        }

        // Compute dab dimensions preserving tip aspect ratio.
        // The `size` input (0-1) scales the longer axis up to MAX_DAB_SIZE;
        // the shorter axis follows from the tip's natural aspect ratio.
        let max = MAX_DAB_SIZE as f32;
        let (tip_w, tip_h) = gpu.dab_pool.tip_size(tip_name).unwrap_or((1, 1));
        let tip_aspect = tip_w as f32 / tip_h as f32;

        let (dab_w, dab_h) = if tip_aspect >= 1.0 {
            // Wide tip: width is the long axis.
            let w = (size * max).max(1.0);
            let h = (w / tip_aspect).max(1.0);
            (w.ceil().min(max) as u32, h.ceil().min(max) as u32)
        } else {
            // Tall tip: height is the long axis.
            let h = (size * max).max(1.0);
            let w = (h * tip_aspect).max(1.0);
            (w.ceil().min(max) as u32, h.ceil().min(max) as u32)
        };

        // Scatter: offset in pixels proportional to the larger dab dimension.
        let dab_major = dab_w.max(dab_h) as f32;
        let scatter_px_x = scatter_x * dab_major;
        let scatter_px_y = scatter_y * dab_major;

        // Rotation: 0-1 maps to 0-2π radians.
        let rotation_rad = rotation_input * std::f32::consts::TAU;

        // Mirror: threshold at 0.5.
        let mirror_x = if mirror_x_input > 0.5 { 1.0 } else { 0.0 };
        let mirror_y = if mirror_y_input > 0.5 { 1.0 } else { 0.0 };

        // Acquire a dab texture from the pool (mutable borrow ends here).
        let handle = gpu.dab_pool.acquire(gpu.device);
        let dab_view = gpu.dab_pool.view(handle);

        // Now safe to borrow immutably for the tip bind group.
        let tip_bind_group = gpu.dab_pool.tip_bind_group(tip_name).unwrap();

        // Write uniforms.
        let uniforms = StampUniforms {
            dab_width: dab_w as f32,
            dab_height: dab_h as f32,
            opacity,
            rotation: rotation_rad,
            color,
            mirror_x,
            mirror_y,
            application: application_int,
            ratio,
        };
        gpu.pipelines.write_stamp_uniforms(gpu.queue, &uniforms);

        // Render stamp to dab texture (non-square viewport).
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-stamp"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dab_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            pass.set_viewport(0.0, 0.0, dab_w as f32, dab_h as f32, 0.0, 1.0);
            pass.set_pipeline(gpu.pipelines.stamp_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.stamp_uniform_bind_group, &[]);
            pass.set_bind_group(1, tip_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        vec![
            ("dab".into(), ScalarValue::Texture(handle)),
            ("dab_size".into(), ScalarValue::Vec2([dab_w as f32, dab_h as f32])),
            ("scatter_offset".into(), ScalarValue::Vec2([scatter_px_x, scatter_px_y])),
        ]
    }
}
