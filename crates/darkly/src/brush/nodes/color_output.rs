//! Color output GPU terminal node.
//!
//! Reads a dab texture and position, then composites the dab onto the
//! canvas layer via a GPU render pass with alpha-over blending.
//! This is the final node in a brush graph — it produces visible output.

use crate::brush::dab_pool::MAX_DAB_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::pipelines::CompositeUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "color_output",
        category: "gpu",
        display_name: "Color Output",
        ports: vec![
            PortDef::input("dab", BrushWireType::Texture),
            PortDef::input("dab_size", BrushWireType::Scalar),
            PortDef::input("position", BrushWireType::Vec2),
        ],
        params: &[],
        is_gpu: true,
    }
}

pub struct ColorOutputEvaluator;

impl BrushNodeEvaluator for ColorOutputEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        // GPU node — CPU evaluation is a no-op.
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let dab_handle = match ctx.input("dab") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let dab_diameter = ctx.input_f32("dab_size");
        let position = ctx.input("position").as_vec2();

        if dab_diameter <= 0.0 {
            return vec![];
        }

        // Position the composite quad centered on the dab position.
        let half = dab_diameter * 0.5;
        let x0 = (position[0] - half).max(0.0);
        let y0 = (position[1] - half).max(0.0);
        let x1 = (position[0] + half).min(gpu.canvas_width as f32);
        let y1 = (position[1] + half).min(gpu.canvas_height as f32);

        let quad_w = x1 - x0;
        let quad_h = y1 - y0;
        if quad_w <= 0.0 || quad_h <= 0.0 {
            return vec![];
        }

        // UV mapping: the dab was rendered to the top-left dab_diameter x dab_diameter
        // region of the pool texture.  Map the quad UVs accordingly.
        let tex_size = MAX_DAB_SIZE as f32;
        let uv_max_x = quad_w / tex_size;
        let uv_max_y = quad_h / tex_size;

        // If the quad was clipped (dab extends beyond canvas edge), offset the UV
        // to skip the clipped portion of the dab texture.
        // TODO: handle UV offset for edge clipping (minor visual artifact at canvas edges)

        let uniforms = CompositeUniforms {
            origin: [x0, y0],
            size: [quad_w, quad_h],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            uv_max: [uv_max_x, uv_max_y],
        };
        gpu.pipelines.write_composite_uniforms(gpu.queue, &uniforms);

        let dab_bind_group = gpu.dab_pool.bind_group(dab_handle);

        // Composite dab onto canvas.
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: gpu.canvas_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            pass.set_viewport(
                0.0, 0.0,
                gpu.canvas_width as f32, gpu.canvas_height as f32,
                0.0, 1.0,
            );
            pass.set_pipeline(gpu.pipelines.composite_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.composite_uniform_bind_group, &[]);
            pass.set_bind_group(1, dab_bind_group, &[]);
            pass.set_bind_group(2, gpu.selection_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // Terminal node — no outputs.
        vec![]
    }
}
