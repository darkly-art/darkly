//! Color output GPU terminal node — paint semantics.
//!
//! `color_output` is the paint terminal of a brush graph. It owns three
//! lifecycle hooks that together define how a paint stroke maps to layer
//! state:
//!
//! 1. `begin_stroke` — clears the stroke scratch to transparent. Called at
//!    stroke start and on every rewind boundary.
//! 2. `evaluate_gpu` (per dab) — composites the dab into the scratch with
//!    straight-alpha Porter-Duff source-over, modulated by the selection
//!    mask. Dabs accumulate, selection masks once. The scratch holds the
//!    stroke's contribution-so-far, selection-already-applied.
//! 3. `commit` (per pen event) — composites the scratch onto the pre-stroke
//!    layer snapshot and writes the result back to the layer. Applies the
//!    stroke-level `opacity` input port as a cap and honours the engine's
//!    paint-vs-erase `blend_mode`. Selection is NOT re-applied (already
//!    baked into the scratch, applying twice would yield `sel²`).
//!
//! The per-dab composite always writes REPLACE with source-over into the
//! scratch — per-dab blend_mode selection doesn't exist, and wouldn't make
//! physical sense (erasing a dab against an empty scratch is a no-op).
//! Engine-level paint-vs-erase is a *stroke* decision, applied at commit.

use crate::brush::dab_pool::MAX_DAB_SIZE;
use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::gpu_context::{BrushGpuContext, RenderMode};
use crate::brush::pipelines::CompositeUniforms;
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef, UnitType};

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
            PortDef::input("opacity", BrushWireType::Scalar)
                .with_range(0.0, 1.0, 1.0)
                .with_label("Opacity")
                .with_unit(UnitType::Percent)
                .with_icon("fa-solid fa-fill-drip")
                .exposed()
                .with_description("Stroke-level opacity cap (max coverage regardless of overlap)"),
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
        // Stroke-only terminal. Preview passes run the same graph but
        // route output into `preview_output` instead.
        if gpu.render_mode != RenderMode::Stroke {
            return vec![];
        }

        let dab_handle = match ctx.input("dab") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let dab_size = ctx.input("dab_size").as_vec2();
        let base_position = ctx.input("position").as_vec2();
        let scatter = ctx.input("scatter_offset").as_vec2();
        let position = [base_position[0] + scatter[0], base_position[1] + scatter[1]];

        let dab_w = dab_size[0];
        let dab_h = dab_size[1];
        if dab_w <= 0.0 || dab_h <= 0.0 {
            return vec![];
        }

        let foot_w = dab_w;
        let foot_h = dab_h;

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

        // Ensure the scratch region under the dab is in canvas_copy for the
        // shader's straight-alpha Porter-Duff read. The bg here is the
        // scratch (not the layer) — source-over against the running stroke
        // accumulation.
        gpu.ensure_canvas_copy(copy_x, copy_y, copy_w, copy_h);

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
            // Per-dab: always source-over. Paint-vs-erase is a stroke-level
            // decision, applied in commit.
            blend_mode: 0,
            fg_premultiplied: 1, // dab from stamp shader is premultiplied
            stroke_opacity: 1.0, // per-dab composites aren't opacity-capped
            apply_selection: 1,  // selection masks every dab as it lands
        };
        let offset = gpu.pipelines.write_composite_uniforms(gpu.queue, &uniforms);

        let dab_bind_group = gpu.dab_pool.bind_group(dab_handle);

        // Composite dab onto the stroke scratch (REPLACE blend — shader does
        // Porter-Duff). The "bg" bind group is canvas_copy, which was just
        // filled with the scratch's current contents above.
        {
            let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("brush-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: gpu.stroke_scratch_view,
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
            pass.set_bind_group(0, &gpu.pipelines.composite_uniform_bind_group, &[offset]);
            pass.set_bind_group(1, dab_bind_group, &[]);
            pass.set_bind_group(2, gpu.selection_bind_group, &[]);
            pass.set_bind_group(3, &gpu.pipelines.canvas_copy_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }

        // Terminal node — no outputs.
        vec![]
    }

    /// Clear the stroke scratch to transparent. Paint starts from an empty
    /// accumulator — per-dab composites pile up from nothing.
    fn begin_stroke(&self, _ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.render_mode != RenderMode::Stroke {
            return;
        }
        let _ = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("color_output-begin_stroke"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: gpu.stroke_scratch_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }

    /// Composite the scratch (= this stroke's accumulated contribution,
    /// already selection-masked) onto the pre-stroke layer snapshot, and
    /// write the result to the layer. Applies the stroke-level `opacity`
    /// port and honours the engine's `blend_mode` (paint vs erase).
    fn commit(&self, ctx: &EvalContext, gpu: &mut BrushGpuContext) {
        if gpu.render_mode != RenderMode::Stroke {
            return;
        }
        // Everything we need must be present; if any piece is missing we're
        // in a pre-refactor fallback path that composites directly to the
        // layer — nothing for commit to do.
        let (Some(layer_view), Some(scratch_bg), Some(pre_stroke_bg)) = (
            gpu.layer_view,
            gpu.scratch_bind_group,
            gpu.pre_stroke_bind_group,
        ) else { return };

        let opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);
        let w = gpu.canvas_width as f32;
        let h = gpu.canvas_height as f32;

        let uniforms = CompositeUniforms {
            origin: [0.0, 0.0],
            size: [w, h],
            canvas_size: [w, h],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            blend_mode: gpu.blend_mode, // paint = 0, erase = 1
            fg_premultiplied: 0,        // scratch is straight alpha
            stroke_opacity: opacity,
            // Selection is already baked into the scratch via per-dab
            // composites — applying again would give sel².
            apply_selection: 0,
        };
        let offset = gpu.pipelines.write_composite_uniforms(gpu.queue, &uniforms);

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("color_output-commit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: layer_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(0.0, 0.0, w, h, 0.0, 1.0);
        pass.set_pipeline(gpu.pipelines.composite_pipeline());
        pass.set_bind_group(0, &gpu.pipelines.composite_uniform_bind_group, &[offset]);
        pass.set_bind_group(1, scratch_bg, &[]);
        pass.set_bind_group(2, gpu.selection_bind_group, &[]);
        pass.set_bind_group(3, pre_stroke_bg, &[]);
        pass.draw(0..6, 0..1);
    }
}
