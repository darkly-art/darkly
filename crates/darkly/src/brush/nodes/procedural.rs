//! Procedural dab generation GPU node.
//!
//! Reads size, softness, opacity, and color from CPU upstream nodes,
//! acquires a dab texture from the pool, and renders an SDF circle
//! via a GPU render pass.  Outputs the texture handle and the actual
//! dab diameter (in pixels) for the composite node.

use crate::brush::dab_pool::MAX_DAB_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::pipelines::DabUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "procedural",
        category: "gpu",
        display_name: "Procedural Dab",
        ports: vec![
            // Inputs (0-1 normalized, mapped to actual ranges internally).
            PortDef::input("size", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5),
            PortDef::input("softness", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 0.5),
            PortDef::input("opacity", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0),
            PortDef::input("color", BrushWireType::Color),
            // Outputs.
            PortDef::output("dab", BrushWireType::Texture),
            PortDef::output("dab_size", BrushWireType::Scalar),
        ],
        params: &[],
        is_gpu: true,
    }
}

pub struct ProceduralEvaluator;

impl BrushNodeEvaluator for ProceduralEvaluator {
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
        let softness = ctx.input_f32("softness");
        let opacity = ctx.input_f32("opacity");
        let color = ctx.input("color").as_color();

        // Map 0-1 size to pixel radius.  size=0 → 0.5px, size=1 → max/2 px.
        let max = MAX_DAB_SIZE as f32;
        let radius = (size * max * 0.5).max(0.5);

        // Softness: 0-1 maps to 0-radius fraction, minimum 1px for AA.
        let softness_px = (softness * radius).max(1.0);

        // Actual dab diameter: tight to the SDF coverage region.
        let dab_diameter = ((2.0 * radius + 2.0).ceil() as u32).min(MAX_DAB_SIZE);

        // Acquire a dab texture from the pool.
        let handle = gpu.dab_pool.acquire(gpu.device);
        let dab_view = gpu.dab_pool.view(handle);

        // Write uniforms.
        let uniforms = DabUniforms {
            dab_size: dab_diameter as f32,
            radius,
            softness: softness_px,
            opacity,
            color,
        };
        gpu.pipelines.write_dab_uniforms(gpu.queue, &uniforms);

        // Render SDF to dab texture.
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-procedural"),
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

            pass.set_viewport(0.0, 0.0, dab_diameter as f32, dab_diameter as f32, 0.0, 1.0);
            pass.set_pipeline(gpu.pipelines.procedural_pipeline());
            pass.set_bind_group(0, &gpu.pipelines.procedural_uniform_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        vec![
            ("dab".into(), ScalarValue::Texture(handle)),
            ("dab_size".into(), ScalarValue::Scalar(dab_diameter as f32)),
        ]
    }
}
