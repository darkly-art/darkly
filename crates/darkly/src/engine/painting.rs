//! Stroke lifecycle, flood fill, erase helpers, and paint infrastructure.

use super::{DarklyEngine, GpuStrokeState, ReadbackContext};
use super::types::StrokeOp;
use crate::gpu::flood_fill;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback;
use crate::undo::GpuRegionAction;

impl DarklyEngine {
    // --- Painting ---

    pub fn fill_gradient(&mut self, layer_id: u64) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let rect = [0, 0, canvas_w, canvas_h];
        let format = wgpu::TextureFormat::Rgba8Unorm;

        let layer_tex = match self.compositor.layer_texture(layer_id) {
            Some(t) => t,
            None => return,
        };

        // Save current state to scratch for undo.
        self.gpu.encode("fill-gradient-save", |encoder| {
            self.region_store.save_region(encoder, &layer_tex.texture, format, rect);
            let entry = self.region_store.commit_region(encoder, layer_id, format, rect);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });

        // Render gradient via GPU paint target.
        let layer_tex = self.compositor.layer_texture(layer_id).unwrap();
        let target = GpuPaintTarget::from_layer(layer_tex, canvas_w, canvas_h);
        self.gpu.encode("fill-gradient-render", |encoder| {
            target.linear_gradient(
                encoder, &self.paint_pipelines, &self.gpu.queue,
                0.0, 0.0, canvas_w as f32, canvas_h as f32,
                [0, 0, 0, 255], [255, 255, 255, 255],
                None,
            );
        });

        self.compositor.mark_dirty();
    }

    // --- Stroke lifecycle ---
    // Following GIMP's edit_mask flag: when editing_mask_layer is set,
    // strokes are routed to the mask instead of the layer.
    //
    // All stroke ops go through GPU render passes (Phase 3).

    pub fn begin_stroke(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        self.doc.set_mask_editing(
            if self.editing_mask_layer == Some(layer_id) { Some(layer_id) } else { None }
        );
        self.active_stroke_layer = Some(layer_id);
        // GPU setup is deferred to first stroke_to (lazy init).
    }

    pub fn stroke_to(&mut self, op: StrokeOp) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };
        self.gpu_stroke_to(layer_id, op);
    }

    /// GPU paint path for all stroke operations.
    fn gpu_stroke_to(&mut self, layer_id: u64, op: StrokeOp) {
        let mask_editing = self.editing_mask_layer == Some(layer_id);
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        // Lazy init: save the region on first stroke_to.
        if self.gpu_stroke.is_none() {
            let (texture, format) = if mask_editing {
                match self.compositor.mask_texture(layer_id) {
                    Some(t) => (&t.texture, wgpu::TextureFormat::R8Unorm),
                    None => return,
                }
            } else {
                match self.compositor.layer_texture(layer_id) {
                    Some(t) => (&t.texture, wgpu::TextureFormat::Rgba8Unorm),
                    None => return,
                }
            };

            // Save the entire canvas to scratch for undo.
            self.gpu.encode("stroke-begin", |encoder| {
                self.region_store.save_region(encoder, texture, format, [0, 0, canvas_w, canvas_h]);
            });

            self.gpu_stroke = Some(GpuStrokeState::new(format));
        }

        // Helper closure to create a paint target from compositor textures.
        // Defined as a macro to avoid holding borrows across match arms.
        macro_rules! paint_target {
            () => {
                if mask_editing {
                    self.compositor.mask_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_mask(t, canvas_w, canvas_h))
                } else {
                    self.compositor.layer_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_layer(t, canvas_w, canvas_h))
                }
            };
        }

        match op {
            StrokeOp::PaintCircle { x, y, radius, r, g, b, a } => {
                let target = match paint_target!() { Some(t) => t, None => return };
                self.gpu.encode("stroke-to", |encoder| {
                    target.composite_circle(
                        encoder, &self.paint_pipelines, &self.gpu.queue,
                        x, y, radius, [r, g, b, a], 1.0,
                    );
                });
                if let Some(gs) = &mut self.gpu_stroke {
                    gs.expand(x, y, radius, canvas_w, canvas_h);
                }
            }
            StrokeOp::EraseCircle { x, y, radius } => {
                let target = match paint_target!() { Some(t) => t, None => return };
                self.gpu.encode("stroke-to", |encoder| {
                    target.erase_circle(
                        encoder, &self.paint_pipelines, &self.gpu.queue,
                        x, y, radius,
                    );
                });
                if let Some(gs) = &mut self.gpu_stroke {
                    gs.expand(x, y, radius, canvas_w, canvas_h);
                }
            }
            StrokeOp::LinearGradient { x0, y0, x1, y1, r0, g0, b0, a0, r1, g1, b1, a1 } => {
                let target = match paint_target!() { Some(t) => t, None => return };
                self.gpu.encode("stroke-gradient", |encoder| {
                    target.linear_gradient(
                        encoder, &self.paint_pipelines, &self.gpu.queue,
                        x0, y0, x1, y1, [r0, g0, b0, a0], [r1, g1, b1, a1], None,
                    );
                });
                // Gradient covers the full canvas.
                if let Some(gs) = &mut self.gpu_stroke {
                    gs.stroke_rect = Some([0, 0, canvas_w, canvas_h]);
                }
            }
            StrokeOp::FloodFill { x, y, r, g, b, a, tolerance } => {
                // Flood fill needs mutable self access, so the target is obtained inside.
                self.gpu_flood_fill(layer_id, mask_editing,
                    x as i32, y as i32, [r, g, b, a], tolerance,
                    canvas_w, canvas_h);
            }
        }

        self.compositor.mark_dirty();
    }

    /// Start async GPU flood fill: readback layer texture, then complete on a
    /// subsequent frame when the data arrives.
    fn gpu_flood_fill(
        &mut self,
        layer_id: u64,
        mask_editing: bool,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
        canvas_w: u32,
        canvas_h: u32,
    ) {
        let (texture, format) = if mask_editing {
            match self.compositor.mask_texture(layer_id) {
                Some(t) => (&t.texture, wgpu::TextureFormat::R8Unorm),
                None => return,
            }
        } else {
            match self.compositor.layer_texture(layer_id) {
                Some(t) => (&t.texture, wgpu::TextureFormat::Rgba8Unorm),
                None => return,
            }
        };

        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("flood-fill-readback") },
        );
        let request = readback::request_readback(
            &self.gpu.device, &mut encoder, texture, format,
            [0, 0, canvas_w, canvas_h],
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.readbacks.submit(request, ReadbackContext::FloodFill {
            layer_id, mask_editing, seed_x, seed_y, color, tolerance, canvas_w, canvas_h,
        });
    }

    /// Complete a pending flood fill once readback data is available.
    pub(crate) fn complete_flood_fill(
        &mut self, layer_id: u64, mask_editing: bool, seed_x: i32, seed_y: i32,
        color: [u8; 4], tolerance: u8, canvas_w: u32, canvas_h: u32, pixels: Vec<u8>,
    ) {
        // 1. CPU scanline fill → produce R8 mask.
        let fill_mask = if mask_editing {
            flood_fill::flood_fill_r8(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
        } else {
            flood_fill::flood_fill_rgba(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
        };

        // 2. Upload fill mask and stamp onto target.
        let mask_bind_group = self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device, &self.gpu.queue, canvas_w, canvas_h,
            &fill_mask, "flood-fill-mask",
        );

        let (target, _) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        self.gpu.encode("flood-fill-stamp", |encoder| {
            target.fill_rect_with_selection(
                encoder, &self.paint_pipelines, &self.gpu.queue,
                [0, 0, canvas_w, canvas_h], color, &mask_bind_group,
            );
        });

        // 4. Commit undo — the stroke lifecycle was deferred for async fill.
        if let Some(gs) = self.gpu_stroke.take() {
            let rect = [0u32, 0, canvas_w, canvas_h];
            self.gpu.encode("flood-fill-undo", |encoder| {
                let entry = self.region_store.commit_region(
                    encoder, layer_id, gs.format, rect,
                );
                self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
            });
        }

        self.compositor.mark_dirty();
    }

    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            // If a flood fill is pending, defer undo commit — complete_flood_fill
            // will handle it when the readback arrives.
            if self.readbacks.any(|c| matches!(c, ReadbackContext::FloodFill { .. })) {
                self.doc.set_mask_editing(None);
                return;
            }

            if let Some(gs) = self.gpu_stroke.take() {
                // GPU path: commit the changed region to the undo buffer.
                if let Some(rect) = gs.stroke_rect {
                    self.gpu.encode("stroke-end", |encoder| {
                        let entry = self.region_store.commit_region(
                            encoder, layer_id, gs.format, rect,
                        );
                        self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
                    });
                }
                // else: no paint was applied (empty stroke), nothing to undo.
            }
            self.doc.set_mask_editing(None);
        }
    }

    // --- GPU erase helpers ---

    /// Clear layer pixels within the current selection via GPU erase pass.
    pub(crate) fn gpu_clear_selection(&mut self, layer_id: u64) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let mask_editing = self.editing_mask_layer == Some(layer_id);

        let (target, format) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        // Upload selection mask as R8 GPU texture.
        let sel_bind_group = match self.upload_selection_mask(canvas_w, canvas_h) {
            Some(bg) => bg,
            None => return,
        };

        // Save region for undo.
        self.gpu.encode("clear-sel-save", |encoder| {
            self.region_store.save_region(encoder, target.texture, format, [0, 0, canvas_w, canvas_h]);
        });

        // Erase within selection.
        let (target, _) = self.get_paint_target(layer_id, mask_editing).unwrap();
        self.gpu.encode("clear-sel-erase", |encoder| {
            target.erase_with_selection(
                encoder, &self.paint_pipelines, &self.gpu.queue, &sel_bind_group,
            );
        });

        // Commit for undo.
        self.gpu.encode("clear-sel-commit", |encoder| {
            let entry = self.region_store.commit_region(
                encoder, layer_id, format, [0, 0, canvas_w, canvas_h],
            );
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
        self.compositor.mark_dirty();
    }

    /// Clear entire layer to transparent via GPU.
    pub(crate) fn gpu_clear_layer(&mut self, layer_id: u64) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let mask_editing = self.editing_mask_layer == Some(layer_id);

        let (target, format) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        // Save region for undo.
        self.gpu.encode("clear-layer-save", |encoder| {
            self.region_store.save_region(encoder, target.texture, format, [0, 0, canvas_w, canvas_h]);
        });

        // Clear the full canvas.
        let (target, _) = self.get_paint_target(layer_id, mask_editing).unwrap();
        self.gpu.encode("clear-layer", |encoder| {
            target.clear_rect(
                encoder, &self.paint_pipelines, &self.gpu.queue,
                [0, 0, canvas_w, canvas_h],
            );
        });

        // Commit for undo.
        self.gpu.encode("clear-layer-commit", |encoder| {
            let entry = self.region_store.commit_region(
                encoder, layer_id, format, [0, 0, canvas_w, canvas_h],
            );
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
        self.compositor.mark_dirty();
    }

    /// Get a GpuPaintTarget for a layer (or its mask), plus its format.
    pub(crate) fn get_paint_target(&self, layer_id: u64, mask_editing: bool) -> Option<(GpuPaintTarget<'_>, wgpu::TextureFormat)> {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        if mask_editing {
            self.compositor.mask_texture(layer_id)
                .map(|t| (GpuPaintTarget::from_mask(t, canvas_w, canvas_h), wgpu::TextureFormat::R8Unorm))
        } else {
            self.compositor.layer_texture(layer_id)
                .map(|t| (GpuPaintTarget::from_layer(t, canvas_w, canvas_h), wgpu::TextureFormat::Rgba8Unorm))
        }
    }

    /// Upload a cropped region of the selection mask as an R8 GPU texture.
    pub(crate) fn upload_cropped_selection_mask(
        &self,
        origin: (i32, i32),
        width: u32,
        height: u32,
    ) -> Option<wgpu::BindGroup> {
        let selection = self.doc.selection.as_ref()?;
        let pixels = selection.rasterize_r8(origin, width, height, 0);
        Some(self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device, &self.gpu.queue, width, height,
            &pixels, "selection-cropped",
        ))
    }

    /// Upload the document's selection mask (AlphaMask) as an R8 GPU texture,
    /// returning a bind group suitable for the paint pipeline's selection slot.
    pub(crate) fn upload_selection_mask(&self, canvas_w: u32, canvas_h: u32) -> Option<wgpu::BindGroup> {
        let selection = self.doc.selection.as_ref()?;
        let pixels = selection.rasterize_r8((0, 0), canvas_w, canvas_h, 0);
        Some(self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device, &self.gpu.queue, canvas_w, canvas_h,
            &pixels, "selection-upload",
        ))
    }
}
