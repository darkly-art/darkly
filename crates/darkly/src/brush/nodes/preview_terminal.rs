//! Internal-only terminal node for per-node previews.
//!
//! Used by the per-node preview pipeline ([`crate::brush::preview_subgraph`])
//! to render any GPU node's `texture` output as a thumbnail. The preview
//! pipeline takes the active brush graph, prunes it to a target node's
//! transitive predecessors, and appends a `preview_terminal` connected to
//! the target's first `Texture` output. The synthesised graph is then run
//! through `BrushPreviewRenderer.render_stroke` exactly like the editor
//! preview, so the readback path and caching are reused verbatim.
//!
//! Shape:
//! - One input port `texture: Texture` (the upstream node's output).
//! - No output ports (terminal).
//! - `category: "internal"` so the frontend palette filters it out and users
//!   can't place one manually.
//! - `evaluate_gpu` blits the input texture stretched to fill the stroke
//!   scratch viewport via the existing `blit_pipeline`. No source-over, no
//!   pre-stroke compositing — previews render to a clean transparent
//!   backdrop.
//! - `commit` straight-blits scratch → layer using `commit_scratch_blit`,
//!   the same path liquify and watercolor use.
//!
//! Why a real graph node and not a special case in the engine: every other
//! GPU terminal in the system implements the same lifecycle (`begin_stroke`,
//! `evaluate_gpu`, `commit`), and `BrushPreviewRenderer` already drives that
//! lifecycle correctly. Fitting the preview as a node keeps the runner and
//! the readback plumbing untouched.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_target_ext::BrushPaintTargetExt;
use crate::brush::pipeline::BlitUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(NodeRegistration {
        type_id: "preview_terminal",
        category: "internal",
        display_name: "Preview Terminal",
        ports: vec![PortDef::input("texture", BrushWireType::Texture)
            .with_description("Texture to render as the per-node preview thumbnail")],
        params: &[],
        is_gpu: true,
    })
}

pub struct PreviewTerminalEvaluator;

impl BrushNodeEvaluator for PreviewTerminalEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let texture_handle = match ctx.input("texture") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };

        let Some(pt) = gpu.paint_target.as_ref() else {
            return vec![];
        };
        let pt_ext = pt.layer_extent();
        let target_w = pt_ext.width as f32;
        let target_h = pt_ext.height as f32;
        if target_w <= 0.0 || target_h <= 0.0 {
            return vec![];
        }

        // Blit the entire source UV range stretched to fill the scratch.
        // Dab pool textures from `acquire_sized` always have content covering
        // [0,1] in UV (one allocation per requested size), so a static
        // [0,1]² rect is universally correct here.
        let uniforms = BlitUniforms {
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
        };
        let offset = gpu.pipelines.write_blit_uniforms(gpu.queue, &uniforms);
        let bg = gpu.dab_pool.bind_group(texture_handle).clone();
        let scratch = gpu
            .scratch
            .as_deref()
            .expect("preview_terminal::evaluate_gpu requires Scratch");

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("preview_terminal-blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: scratch.write_view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(0.0, 0.0, target_w, target_h, 0.0, 1.0);
        pass.set_pipeline(gpu.pipelines.blit_pipeline());
        pass.set_bind_group(0, &gpu.pipelines.blit_uniform_bind_group, &[offset]);
        pass.set_bind_group(1, &bg, &[]);
        pass.draw(0..3, 0..1);

        vec![]
    }

    /// Direct scratch → layer blit, same shape as liquify and watercolor's
    /// commit. The scratch already holds the finished thumbnail (the
    /// `evaluate_gpu` blit above), so commit just copies it across.
    fn commit(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        let Some(paint_target) = gpu.paint_target.as_ref() else {
            return;
        };
        let Some(scratch) = gpu.scratch.as_deref() else {
            return;
        };
        paint_target.commit_scratch_blit(
            gpu.device,
            &mut gpu.encoder,
            gpu.pipelines,
            scratch.write_view(),
            scratch.write_texture(),
        );
    }
}
