//! Preview output GPU sink node.
//!
//! Symmetric to `color_output` but writes into the overlay's preview mask
//! texture instead of the canvas. A brush graph has two sinks: the
//! stroke path feeds `color_output`, and the preview path feeds
//! `preview_output`. The engine triggers a preview regen by running the
//! graph with `render_mode: Preview`, which causes `color_output` to
//! bail and `preview_output` to blit.
//!
//! Implementation detail: the upstream dab texture is a full `MAX_DAB_SIZE`
//! pool texture with content only in a (dab_w × dab_h) top-left corner.
//! This node uses the `blit_pipeline` to sample that sub-rectangle via
//! `uv_max = (dab_w / MAX_DAB_SIZE, dab_h / MAX_DAB_SIZE)` and stretches
//! it across the preview target. Canvas-space positioning for the overlay
//! primitive is not computed here — the engine reads this node's resolved
//! `dab_size` and `rotation` slot values after GPU eval.

use crate::brush::dab_pool::MAX_DAB_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, RenderMode};
use crate::brush::pipelines::BlitUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "preview_output",
        category: "gpu",
        display_name: "Preview Output",
        ports: vec![
            PortDef::input("dab", BrushWireType::Texture)
                .with_description("Rendered dab to show as the hover preview mask"),
            PortDef::input("dab_size", BrushWireType::Vec2)
                .with_description("Dab dimensions in canvas pixels — drives the overlay primitive's half-extent"),
            PortDef::input("rotation", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.0)
                .with_description("Brush rotation (0-1 maps to 0-2π) — drives the overlay primitive's rotation"),
        ],
        params: &[],
        is_gpu: true,
    }
}

pub struct PreviewOutputEvaluator;

impl BrushNodeEvaluator for PreviewOutputEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        if gpu.render_mode != RenderMode::Preview {
            return vec![];
        }
        let Some(target_view) = gpu.preview_mask_view else { return vec![]; };
        let (target_w, target_h) = gpu.preview_mask_size;

        let dab_handle = match ctx.input("dab") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let dab_size = ctx.input("dab_size").as_vec2();
        if dab_size[0] <= 0.0 || dab_size[1] <= 0.0 {
            return vec![];
        }

        // Dab pool textures are MAX_DAB_SIZE × MAX_DAB_SIZE; the actual
        // content lives in the top-left (dab_w × dab_h) subrect.
        let tex_dim = MAX_DAB_SIZE as f32;
        let uniforms = BlitUniforms {
            uv_min: [0.0, 0.0],
            uv_max: [dab_size[0] / tex_dim, dab_size[1] / tex_dim],
        };
        let offset = gpu.pipelines.write_blit_uniforms(gpu.queue, &uniforms);
        let dab_bind_group = gpu.dab_pool.bind_group(dab_handle).clone();

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("brush-preview-blit"),
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
        pass.set_viewport(0.0, 0.0, target_w as f32, target_h as f32, 0.0, 1.0);
        pass.set_pipeline(gpu.pipelines.blit_pipeline());
        pass.set_bind_group(0, &gpu.pipelines.blit_uniform_bind_group, &[offset]);
        pass.set_bind_group(1, &dab_bind_group, &[]);
        pass.draw(0..3, 0..1);

        vec![]
    }
}
