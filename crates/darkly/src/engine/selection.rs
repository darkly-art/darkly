//! Selection operations — rect, ellipse, lasso, magic wand, clear, invert.
//!
//! All selection state is GPU-authoritative. Tools rasterize shapes to flat
//! R8 buffers, which are uploaded directly to the GPU selection texture.
//! Boolean ops (add/subtract/intersect) run as GPU shader passes.

use super::{DarklyEngine, ReadbackContext};
use crate::document::SelectionMode;
use crate::engine::gpu_selection::CombineMode;
use crate::gpu::flood_fill;
use crate::gpu::overlay::{OverlayPrimitive, KIND_DASHED_LINE, FLAG_CANVAS_SPACE};
use crate::gpu::readback;
use crate::undo::SelectionAction;

impl DarklyEngine {
    pub fn select_rect(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let half_w = w * 0.5;
        let half_h = h * 0.5;

        let pixels = crate::mask::rasterize_sdf_r8(
            self.doc.width, self.doc.height,
            (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
            |px, py| crate::sdf::sdf_rect(px, py, cx, cy, half_w, half_h),
            antialias, feather,
        );
        self.apply_selection_r8(pixels, mode);
    }

    pub fn select_ellipse(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let rx = w * 0.5;
        let ry = h * 0.5;

        let pixels = crate::mask::rasterize_sdf_r8(
            self.doc.width, self.doc.height,
            (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
            |px, py| crate::sdf::sdf_ellipse(px, py, cx, cy, rx, ry),
            antialias, feather,
        );
        self.apply_selection_r8(pixels, mode);
    }

    pub fn select_lasso(
        &mut self,
        vertices: &[[f32; 2]],
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        if vertices.len() < 3 {
            return;
        }

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for v in vertices {
            min_x = min_x.min(v[0]);
            min_y = min_y.min(v[1]);
            max_x = max_x.max(v[0]);
            max_y = max_y.max(v[1]);
        }
        let bx = min_x.floor() as i32;
        let by = min_y.floor() as i32;
        let bw = (max_x - min_x).ceil() as i32 + 1;
        let bh = (max_y - min_y).ceil() as i32 + 1;

        let verts: Vec<[f32; 2]> = vertices.to_vec();
        let pixels = crate::mask::rasterize_sdf_r8(
            self.doc.width, self.doc.height,
            (bx, by, bw, bh),
            |px, py| crate::sdf::sdf_polygon(px, py, &verts),
            antialias, feather,
        );
        self.apply_selection_r8(pixels, mode);
    }

    pub fn select_magic_wand(
        &mut self,
        layer_id: u64,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: SelectionMode,
    ) {
        if self.compositor.layer_texture(layer_id).is_none() {
            return;
        }
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        let was_active = self.gpu_selection.active;

        // Save selection texture for undo BEFORE the mutation.
        self.save_selection_for_undo();

        // Async GPU readback of layer texture.
        let layer_tex = self.compositor.layer_texture(layer_id).unwrap();
        self.gpu.encode("magic-wand-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device, encoder, &layer_tex.texture,
                wgpu::TextureFormat::Rgba8Unorm, [0, 0, canvas_w, canvas_h],
            );
            self.readbacks.submit(request, ReadbackContext::MagicWand {
                was_active, seed_x, seed_y, tolerance, mode,
            });
        });
    }

    /// Complete magic wand after async readback.
    pub(crate) fn complete_magic_wand(
        &mut self, was_active: bool, seed_x: i32, seed_y: i32,
        tolerance: u8, mode: SelectionMode, pixels: Vec<u8>,
    ) {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // CPU scanline flood fill on the flat pixel data.
        let fill_mask = flood_fill::flood_fill_rgba(
            &pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance,
        );

        // Apply to GPU selection.
        self.apply_selection_r8_with_undo(fill_mask, mode, was_active);
    }

    pub fn clear_selection(&mut self) {
        if !self.gpu_selection.active {
            return;
        }
        self.save_selection_for_undo();
        let was_active = self.gpu_selection.active;

        self.gpu_selection.clear(&self.gpu.queue);

        self.commit_selection_undo(was_active);
        self.update_selection_overlay();
    }

    pub fn select_all(&mut self) {
        self.save_selection_for_undo();
        let was_active = self.gpu_selection.active;

        let w = self.doc.width;
        let h = self.doc.height;
        let data = vec![255u8; (w * h) as usize];
        self.gpu_selection.upload_replace(
            &self.gpu.device, &self.gpu.queue, &data,
            self.brush_pipelines.selection_bind_group_layout(),
            &self.paint_pipelines.selection_bind_group_layout,
        );

        self.commit_selection_undo(was_active);
        self.update_selection_overlay();
    }

    pub fn invert_selection(&mut self) {
        if !self.gpu_selection.active {
            return;
        }
        self.save_selection_for_undo();
        let was_active = self.gpu_selection.active;

        self.gpu.encode("invert-sel", |encoder| {
            self.selection_pipelines.invert(
                encoder, &self.gpu.device, &self.gpu.queue,
                &mut self.gpu_selection,
                self.brush_pipelines.selection_bind_group_layout(),
                &self.paint_pipelines.selection_bind_group_layout,
            );
        });
        self.commit_selection_undo(was_active);
        self.kick_selection_readback();
        self.update_selection_overlay();
    }

    pub fn clear_selection_contents(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        if !self.gpu_selection.active {
            return;
        }
        self.gpu_clear_selection(layer_id);
    }

    pub fn has_selection(&self) -> bool {
        self.gpu_selection.active
    }

    // --- Core selection application ---

    /// Apply a rasterized R8 shape to the GPU selection, with full undo support.
    fn apply_selection_r8(&mut self, shape_pixels: Vec<u8>, mode: SelectionMode) {
        self.save_selection_for_undo();
        let was_active = self.gpu_selection.active;
        self.apply_selection_r8_with_undo(shape_pixels, mode, was_active);
    }

    /// Apply shape after undo has already been saved (used by async paths like magic wand).
    fn apply_selection_r8_with_undo(
        &mut self, shape_pixels: Vec<u8>, mode: SelectionMode, was_active: bool,
    ) {
        match mode {
            SelectionMode::Replace => {
                self.gpu_selection.upload_replace(
                    &self.gpu.device, &self.gpu.queue, &shape_pixels,
                    self.brush_pipelines.selection_bind_group_layout(),
                    &self.paint_pipelines.selection_bind_group_layout,
                );
            }
            _ => {
                let combine_mode = CombineMode::from_selection_mode(&mode);
                self.gpu.encode("sel-combine", |encoder| {
                    self.selection_pipelines.combine(
                        encoder, &self.gpu.device, &self.gpu.queue,
                        &mut self.gpu_selection,
                        &shape_pixels,
                        combine_mode,
                        self.brush_pipelines.selection_bind_group_layout(),
                        &self.paint_pipelines.selection_bind_group_layout,
                    );
                });
                // Boolean ops produce result on GPU — kick readback for CPU cache.
                self.kick_selection_readback();
            }
        }


        self.commit_selection_undo(was_active);
        self.update_selection_overlay();
    }

    // --- Undo helpers ---

    /// Save the current selection GPU texture to the region store scratch.
    pub(crate) fn save_selection_for_undo(&mut self) {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let texture = self.gpu_selection.texture();
        self.gpu.encode("sel-undo-save", |encoder| {
            self.region_store.save_region(
                encoder, texture, wgpu::TextureFormat::R8Unorm,
                [0, 0, canvas_w, canvas_h],
            );
        });
    }

    /// Commit the saved region and push a SelectionAction to the undo stack.
    pub(crate) fn commit_selection_undo(&mut self, was_active: bool) {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        // Use layer_id 0 as sentinel — the engine dispatches via selection_region_entry_mut.
        self.gpu.encode("sel-undo-commit", |encoder| {
            let entry = self.region_store.commit_region(
                encoder, 0, wgpu::TextureFormat::R8Unorm,
                [0, 0, canvas_w, canvas_h],
            );
            self.undo_stack.push(Box::new(SelectionAction::new(was_active, entry)));
        });
    }

    /// Kick an async readback of the GPU selection texture to update the CPU cache.
    pub(crate) fn kick_selection_readback(&mut self) {
        let w = self.doc.width;
        let h = self.doc.height;
        let texture = self.gpu_selection.texture();
        self.gpu.encode("sel-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device, encoder, texture,
                wgpu::TextureFormat::R8Unorm, [0, 0, w, h],
            );
            self.readbacks.submit(request, ReadbackContext::SelectionReadback);
        });
    }

    // --- Selection overlay ---

    /// Regenerate marching ants overlay from the current selection.
    pub(crate) fn update_selection_overlay(&mut self) {
        self.selection_overlay.clear();

        if self.gpu_selection.active && self.gpu_selection.cache_valid {
            let segments = crate::mask::contour_segments_r8(
                &self.gpu_selection.cpu_cache,
                self.gpu_selection.width,
                self.gpu_selection.height,
                127, // threshold at ~0.5
            );
            for (a, b) in &segments {
                let mut bg = OverlayPrimitive::new(
                    KIND_DASHED_LINE,
                    FLAG_CANVAS_SPACE,
                    *a, *b,
                );
                bg.color = [0.0, 0.0, 0.0, 1.0];
                bg.thickness = 1.5;
                bg.dash_len = 0.0;
                self.selection_overlay.push(bg);
            }
            for (a, b) in &segments {
                let mut fg = OverlayPrimitive::new(
                    KIND_DASHED_LINE,
                    FLAG_CANVAS_SPACE,
                    *a, *b,
                );
                fg.color = [1.0, 1.0, 1.0, 1.0];
                fg.thickness = 1.0;
                fg.dash_len = 8.0;
                self.selection_overlay.push(fg);
            }
        }

        self.push_merged_overlay();
    }

    /// Merge selection_overlay + tool_overlay and push to compositor.
    pub(crate) fn push_merged_overlay(&mut self) {
        let mut merged = Vec::with_capacity(self.selection_overlay.len() + self.tool_overlay.len());
        merged.extend_from_slice(&self.selection_overlay);
        merged.extend_from_slice(&self.tool_overlay);
        if merged.is_empty() {
            self.compositor.clear_overlay();
        } else {
            self.compositor.set_overlay_primitives(merged);
        }
    }

    // --- Tool Overlay ---

    pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.tool_overlay = prims;
        self.push_merged_overlay();
    }

    pub fn clear_overlay(&mut self) {
        self.tool_overlay.clear();
        self.push_merged_overlay();
    }

    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.compositor.overlay_hit_test(screen_x, screen_y)
    }
}
