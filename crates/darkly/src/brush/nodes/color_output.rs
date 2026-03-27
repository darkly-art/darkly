//! Color output GPU terminal node.
//!
//! Reads a dab texture and position, then composites the dab onto the
//! canvas layer via shader-side Porter-Duff source-over blending.
//! This is the final node in a brush graph — it produces visible output.
//!
//! The dab texture contains the brush content rendered at internal resolution
//! (up to `MAX_DAB_SIZE`).  The `global_scale` from `BrushGpuContext` controls
//! the final canvas footprint: the composite quad is sized to
//! `dab_size * global_scale`, and the GPU bilinear samples the dab texture
//! into the larger (or smaller) quad.  This decouples brush size from
//! internal rendering resolution.
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
            PortDef::input("dab", BrushWireType::Texture)
                .with_description("The rendered dab texture to composite onto the canvas"),
            PortDef::input("dab_size", BrushWireType::Vec2)
                .with_description("Width and height of the dab in pixels"),
            PortDef::input("position", BrushWireType::Vec2)
                .with_description("Canvas position where the dab center is placed"),
            PortDef::input("scatter_offset", BrushWireType::Vec2)
                .with_description("Random offset added to the position for scatter effects"),
            PortDef::input("blend_mode", BrushWireType::Int)
                .with_range(0.0, 1.0, 0.0)
                .with_description("Compositing blend mode (0 = source over, 1 = erase)"),
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
        let dab_size = ctx.input("dab_size").as_vec2();
        let base_position = ctx.input("position").as_vec2();
        let scatter = ctx.input("scatter_offset").as_vec2();
        let position = [base_position[0] + scatter[0], base_position[1] + scatter[1]];
        // Port value if explicitly wired, otherwise use engine-level override.
        let port_blend = ctx.input("blend_mode").as_f32() as u32;
        let blend_mode = port_blend.max(gpu.blend_mode);

        let dab_w = dab_size[0];
        let dab_h = dab_size[1];
        if dab_w <= 0.0 || dab_h <= 0.0 {
            return vec![];
        }

        // Apply global_scale to get the canvas footprint.
        let scale = gpu.global_scale;
        let foot_w = dab_w * scale;
        let foot_h = dab_h * scale;

        // Position the composite quad centered on the dab position,
        // clamped to canvas bounds.
        let half_w = foot_w * 0.5;
        let half_h = foot_h * 0.5;
        let unclipped_x0 = position[0] - half_w;
        let unclipped_y0 = position[1] - half_h;
        let x0 = unclipped_x0.max(0.0);
        let y0 = unclipped_y0.max(0.0);
        let x1 = (position[0] + half_w).min(gpu.canvas_width as f32);
        let y1 = (position[1] + half_h).min(gpu.canvas_height as f32);

        let quad_w = x1 - x0;
        let quad_h = y1 - y0;
        if quad_w <= 0.0 || quad_h <= 0.0 {
            return vec![];
        }

        // Integer pixel rect for the canvas copy.  The composite shader
        // uses floor(origin) for the copy UV, so the copy must span from
        // floor(x0) to ceil(x1) to cover every texel the shader can reach.
        let copy_x = x0 as u32;
        let copy_y = y0 as u32;
        let copy_w = ((x1.ceil() as u32).saturating_sub(copy_x))
            .min(gpu.canvas_width - copy_x);
        let copy_h = ((y1.ceil() as u32).saturating_sub(copy_y))
            .min(gpu.canvas_height - copy_y);

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

        // UV mapping: the dab content occupies [0..dab_w] x [0..dab_h] in the
        // MAX_DAB_SIZE pool texture.  The composite quad maps to the scaled
        // canvas footprint.  When clipped at the canvas edge, offset the UV
        // start to skip the clipped portion.
        let tex_w = MAX_DAB_SIZE as f32;
        let tex_h = MAX_DAB_SIZE as f32;
        let content_uv_w = dab_w / tex_w;
        let content_uv_h = dab_h / tex_h;

        // Fraction of the footprint that is clipped on each side.
        let uv_min_x = (x0 - unclipped_x0) / foot_w * content_uv_w;
        let uv_min_y = (y0 - unclipped_y0) / foot_h * content_uv_h;
        let uv_max_x = (x1 - unclipped_x0) / foot_w * content_uv_w;
        let uv_max_y = (y1 - unclipped_y0) / foot_h * content_uv_h;

        let uniforms = CompositeUniforms {
            origin: [x0, y0],
            size: [quad_w, quad_h],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
            uv_min: [uv_min_x, uv_min_y],
            uv_max: [uv_max_x, uv_max_y],
            blend_mode,
            _pad: 0,
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
