//! Color output GPU terminal node — paint semantics.
//!
//! `color_output` is the paint terminal of a brush graph. It owns four
//! lifecycle hooks that together define how a paint stroke maps to layer
//! state and how the brush appears under the hover cursor:
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
//! 4. `render_preview` — reads the `brush_preview` input texture (typically
//!    the brush tip's preview output, but accepts any texture), blits it
//!    into the overlay's preview mask, and publishes placement info.
//!    Deposition settings (flow, opacity, blend mode) are intentionally
//!    *not* consulted — preview shows the brush, not the paint deposit.
//!
//! The per-dab composite always writes REPLACE with source-over into the
//! scratch — per-dab blend_mode selection doesn't exist, and wouldn't make
//! physical sense (erasing a dab against an empty scratch is a no-op).
//! Engine-level paint-vs-erase is a *stroke* decision, applied at commit.

use crate::brush::eval::{BrushNodeEvaluator, BrushPreviewInfo, EvalContext};
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::pipelines::{BlitUniforms, CompositeUniforms};
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
            PortDef::input("brush_preview", BrushWireType::Texture)
                .with_description("Hover-preview texture. Its dimensions are the brush's canvas-pixel extent (rotation, ratio, mirror baked in). Typically wired from a brush tip's preview output, but accepts any texture."),
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
        // Preview path is dispatched via `render_preview`; this method is
        // only invoked during stroke evaluation, no mode check needed.

        let dab_handle = match ctx.input("dab") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let dab_size = ctx.input("dab_size").as_vec2();
        let position = ctx.input("position").as_vec2();

        let dab_w = dab_size[0];
        let dab_h = dab_size[1];
        if dab_w <= 0.0 || dab_h <= 0.0 {
            return vec![];
        }

        let foot_w = dab_w;
        let foot_h = dab_h;

        // Position the composite quad centered on the dab position,
        // clamped to the LAYER's canvas extent. Paste-extent / grown
        // layers may extend past the canvas bounds in either direction —
        // dabs landing on those off-canvas pixels must still render.
        let half_w = foot_w * 0.5;
        let half_h = foot_h * 0.5;
        let unclipped_x0 = position[0] - half_w;
        let unclipped_y0 = position[1] - half_h;
        let layer_x0 = gpu.layer_offset_x as f32;
        let layer_y0 = gpu.layer_offset_y as f32;
        let layer_x1 = layer_x0 + gpu.layer_width as f32;
        let layer_y1 = layer_y0 + gpu.layer_height as f32;
        let x0 = unclipped_x0.max(layer_x0);
        let y0 = unclipped_y0.max(layer_y0);
        let x1 = (position[0] + half_w).min(layer_x1);
        let y1 = (position[1] + half_h).min(layer_y1);

        let quad_w = x1 - x0;
        let quad_h = y1 - y0;
        if quad_w <= 0.0 || quad_h <= 0.0 {
            return vec![];
        }

        // Integer canvas-space rect for the dab's footprint. The composite
        // shader uses floor(origin) for the copy UV, so the copy must span
        // from floor(x0) to ceil(x1) to cover every texel the shader can
        // reach.
        let copy_canvas_x = x0.floor() as i32;
        let copy_canvas_y = y0.floor() as i32;
        let copy_w = (x1.ceil() as i32 - copy_canvas_x) as u32;
        let copy_h = (y1.ceil() as i32 - copy_canvas_y) as u32;

        if copy_w == 0 || copy_h == 0 {
            return vec![];
        }

        // canvas_copy and save-point bboxes both index the stroke scratch,
        // which is layer-sized — translate the canvas-space rect to the
        // layer's local coord frame for both consumers.
        let copy_local_x = (copy_canvas_x - gpu.layer_offset_x) as u32;
        let copy_local_y = (copy_canvas_y - gpu.layer_offset_y) as u32;

        // Publish the dab's layer-local footprint so the stroke engine
        // can record a save-point bbox that matches what was actually
        // drawn. Layer-local matches the scratch coord frame the
        // save_points reference. Authoritative — `info.pos ± dab_radius`
        // isn't, because the graph may offset position (scatter etc.).
        gpu.push_dab_write_bbox([copy_local_x, copy_local_y, copy_w, copy_h]);

        // Ensure the scratch region under the dab is in canvas_copy for the
        // shader's straight-alpha Porter-Duff read. The bg here is the
        // scratch (not the layer) — source-over against the running stroke
        // accumulation.
        gpu.ensure_canvas_copy(copy_local_x, copy_local_y, copy_w, copy_h);

        // UV mapping: the dab content occupies [0..dab_w] x [0..dab_h] within
        // a (tex_w x tex_h) texture allocated by the dab pool. Most stamps
        // size the texture to match the dab exactly (content_uv = 1.0); a
        // mismatch only happens if a node deliberately renders into a
        // larger pool texture. Query the actual size — don't assume
        // MAX_DAB_SIZE.
        let (pool_w, pool_h) = gpu.dab_pool.texture_size(dab_handle);
        let tex_w = pool_w as f32;
        let tex_h = pool_h as f32;
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
            // Per-dab composite renders into stroke_scratch, sized to layer
            // bounds. target_offset is the layer's canvas-space offset.
            target_offset: [gpu.layer_offset_x as f32, gpu.layer_offset_y as f32],
            target_size: [gpu.layer_width as f32, gpu.layer_height as f32],
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

            // Viewport must match the stroke scratch (sized to layer bounds).
            pass.set_viewport(
                0.0,
                0.0,
                gpu.layer_width as f32,
                gpu.layer_height as f32,
                0.0,
                1.0,
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
        let _ = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("color_output-begin_stroke"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: gpu.stroke_scratch_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
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
        // Everything we need must be present; if any piece is missing we're
        // in a pre-refactor fallback path that composites directly to the
        // layer — nothing for commit to do.
        let (Some(layer_view), Some(scratch_bg), Some(pre_stroke_bg)) = (
            gpu.layer_view,
            gpu.scratch_bind_group,
            gpu.pre_stroke_bind_group,
        ) else {
            return;
        };

        let opacity = ctx.input_f32("opacity").clamp(0.0, 1.0);
        // Commit composites the layer-sized scratch onto the layer-sized
        // pre-stroke snapshot, writing to the layer texture. All three are
        // sized to the layer's bounds; use canvas-space coords throughout.
        let layer_w = gpu.layer_width as f32;
        let layer_h = gpu.layer_height as f32;
        let layer_off_x = gpu.layer_offset_x as f32;
        let layer_off_y = gpu.layer_offset_y as f32;

        let uniforms = CompositeUniforms {
            origin: [layer_off_x, layer_off_y],
            size: [layer_w, layer_h],
            target_offset: [layer_off_x, layer_off_y],
            target_size: [layer_w, layer_h],
            canvas_size: [gpu.canvas_width as f32, gpu.canvas_height as f32],
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
        // Viewport must match the render target (the layer texture) so NDC
        // [-1,1] maps to [0, layer_w] × [0, layer_h].
        pass.set_viewport(0.0, 0.0, layer_w, layer_h, 0.0, 1.0);
        pass.set_pipeline(gpu.pipelines.composite_pipeline());
        pass.set_bind_group(0, &gpu.pipelines.composite_uniform_bind_group, &[offset]);
        pass.set_bind_group(1, scratch_bg, &[]);
        pass.set_bind_group(2, gpu.selection_bind_group, &[]);
        pass.set_bind_group(3, pre_stroke_bg, &[]);
        pass.draw(0..6, 0..1);
    }

    /// Render the hover preview into the overlay's preview mask.
    ///
    /// Reads the `brush_preview` input texture (typically wired from the
    /// brush tip's `preview` output, but accepts any texture) and blits it
    /// stretched into the overlay's preview mask via the blit pipeline.
    /// The input texture's own dimensions encode the brush's canvas-pixel
    /// extent — we publish those to the engine via `gpu.brush_preview_info`
    /// so the overlay primitive can size itself correctly. Rotation is
    /// considered baked into the texture (the brush terminal's job), so
    /// `rotation_rad` here is always 0.
    ///
    /// Deposition settings (flow, opacity, blend mode) are intentionally
    /// not consulted — the preview shows the brush, not the per-dab paint
    /// deposit. This is what `evaluate_gpu`'s composite path is for.
    fn render_preview(
        &self,
        ctx: &EvalContext,
        gpu: &mut BrushGpuContext,
    ) -> Vec<(String, ScalarValue)> {
        let preview_handle = match ctx.input("brush_preview") {
            ScalarValue::Texture(h) => h,
            _ => return vec![],
        };
        let Some(target_view) = gpu.preview_mask_view else {
            return vec![];
        };
        let (target_w, target_h) = gpu.preview_mask_size;
        if target_w == 0 || target_h == 0 {
            return vec![];
        }

        // The input texture self-describes its canvas-pixel extent.
        let (extent_w, extent_h) = gpu.dab_pool.texture_size(preview_handle);
        if extent_w == 0 || extent_h == 0 {
            return vec![];
        }

        // Blit the entire input texture (UV [0,1]) stretched into the
        // preview mask. The mask is a fixed-size canvas the overlay shader
        // samples in normalised UV — bilinear filtering takes care of the
        // display-time scaling driven by the primitive's halfExtent.
        let uniforms = BlitUniforms {
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
        };
        let offset = gpu.pipelines.write_blit_uniforms(gpu.queue, &uniforms);
        let bg = gpu.dab_pool.bind_group(preview_handle).clone();

        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("color_output-render_preview"),
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
        pass.set_bind_group(1, &bg, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);

        // Publish placement info: the texture's canvas-pixel extent
        // becomes the overlay primitive's halfExtent. Rotation is baked
        // into the texture, so we report 0 here.
        if gpu.brush_preview_info.is_none() {
            gpu.brush_preview_info = Some(BrushPreviewInfo {
                half_extent_canvas_px: [extent_w as f32 * 0.5, extent_h as f32 * 0.5],
                rotation_rad: 0.0,
            });
        }

        vec![]
    }
}
