//! Circle mask GPU node.
//!
//! Renders an SDF circle to a dab texture — a white mask with soft edges.
//! The stamp node handles sizing, color, rotation, and compositing.
//! This separation means any procedural shape (square, star, polygon)
//! can be swapped in without touching the stamping logic.

use crate::brush::dab_pool::MAX_DAB_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::pipelines::CircleUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "circle",
        category: "gpu",
        display_name: "Circle",
        ports: vec![
            PortDef::input("softness", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5),
            PortDef::output("texture", BrushWireType::Texture),
        ],
        params: &[],
        is_gpu: true,
    }
}

pub struct CircleEvaluator;

impl BrushNodeEvaluator for CircleEvaluator {
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![]
    }

    fn evaluate_gpu(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let softness = ctx.input_f32("softness");

        let handle = gpu.dab_pool.acquire(gpu.device);
        let dab_view = gpu.dab_pool.view(handle);

        let uniforms = CircleUniforms {
            softness,
            _pad: [0.0; 3],
        };
        gpu.pipelines.write_circle_uniforms(gpu.queue, &uniforms);

        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-circle"),
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

            let size = MAX_DAB_SIZE as f32;
            pass.set_viewport(0.0, 0.0, size, size, 0.0, 1.0);
            pass.set_pipeline(gpu.pipelines.circle_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.circle_uniform_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        vec![
            ("texture".into(), ScalarValue::Texture(handle)),
        ]
    }
}
