//! Color output GPU terminal node.
//!
//! Reads a dab texture and position, then composites the dab onto the
//! canvas layer via shader-side Porter-Duff source-over blending.
//! This is the final node in a brush graph — it produces visible output.
//!
//! Before the composite render pass, the canvas region under the dab is
//! copied to a temporary texture.  The shader reads both the dab and the
//! canvas copy, computes correct straight-alpha compositing, and writes
//! the result with REPLACE blend — avoiding the premultiplied-stored-as-
//! straight bug that hardware alpha blending causes on straight-alpha
//! layer textures (see compositing-lessons-learned.md #2).

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

        // Position the composite quad centered on the dab position,
        // clamped to canvas bounds.
        let half = dab_diameter * 0.5;
        let unclipped_x0 = position[0] - half;
        let unclipped_y0 = position[1] - half;
        let x0 = unclipped_x0.max(0.0);
        let y0 = unclipped_y0.max(0.0);
        let x1 = (position[0] + half).min(gpu.canvas_width as f32);
        let y1 = (position[1] + half).min(gpu.canvas_height as f32);

        let quad_w = x1 - x0;
        let quad_h = y1 - y0;
        if quad_w <= 0.0 || quad_h <= 0.0 {
            return vec![];
        }

        // Integer pixel rect for the copy (ceil to cover partial pixels).
        let copy_x = x0 as u32;
        let copy_y = y0 as u32;
        let copy_w = (quad_w.ceil() as u32).min(gpu.canvas_width - copy_x).min(MAX_DAB_SIZE);
        let copy_h = (quad_h.ceil() as u32).min(gpu.canvas_height - copy_y).min(MAX_DAB_SIZE);

        if copy_w == 0 || copy_h == 0 {
            return vec![];
        }

        // Copy the canvas region under the dab to the canvas-copy texture.
        // The shader reads this to do correct straight-alpha Porter-Duff.
        gpu.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: gpu.canvas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: copy_x, y: copy_y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: gpu.pipelines.canvas_copy_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: copy_w,
                height: copy_h,
                depth_or_array_layers: 1,
            },
        );

        // UV mapping: the dab occupies [0, dab_diameter) in the pool texture.
        // When clipped at the top/left canvas edge, offset the UV start to
        // skip the clipped portion of the dab texture.
        let tex_size = MAX_DAB_SIZE as f32;
        let uv_min_x = (x0 - unclipped_x0) / tex_size;
        let uv_min_y = (y0 - unclipped_y0) / tex_size;
        let uv_max_x = (x1 - unclipped_x0) / tex_size;
        let uv_max_y = (y1 - unclipped_y0) / tex_size;

        let uniforms = CompositeUniforms {
            origin: [x0, y0],
            size: [quad_w, quad_h],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            uv_min: [uv_min_x, uv_min_y],
            uv_max: [uv_max_x, uv_max_y],
        };
        gpu.pipelines.write_composite_uniforms(gpu.queue, &uniforms);

        let dab_bind_group = gpu.dab_pool.bind_group(dab_handle);

        // Composite dab onto canvas (REPLACE blend — shader does Porter-Duff).
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
            pass.set_bind_group(3, &gpu.pipelines.canvas_copy_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }

        // Terminal node — no outputs.
        vec![]
    }
}
