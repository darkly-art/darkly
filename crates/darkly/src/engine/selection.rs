//! Selection operations — rect, ellipse, lasso, magic wand, clear, invert.
//!
//! GPU-authoritative. No persistent CPU cache — the GPU texture is truth.
//! Contour extraction (marching ants) runs on async readback data.

use super::{DarklyEngine, ReadbackContext};
use crate::document::SelectionMode;
use crate::engine::gpu_selection::CombineMode;
use crate::gpu::flood_fill;
use crate::gpu::overlay::{OverlayPrimitive, FLAG_CANVAS_SPACE, KIND_DASHED_LINE};
use crate::gpu::readback;
use crate::mask::RasterizedMask;
use crate::undo::SelectionAction;

impl DarklyEngine {
    pub fn select_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let half_w = w * 0.5;
        let half_h = h * 0.5;

        let mask = crate::mask::rasterize_sdf_r8(
            self.doc.width,
            self.doc.height,
            (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
            |px, py| crate::sdf::sdf_rect(px, py, cx, cy, half_w, half_h),
            antialias,
            feather,
        );
        self.apply_selection_mask(mask, mode);
    }

    pub fn select_ellipse(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let rx = w * 0.5;
        let ry = h * 0.5;

        let mask = crate::mask::rasterize_sdf_r8(
            self.doc.width,
            self.doc.height,
            (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
            |px, py| crate::sdf::sdf_ellipse(px, py, cx, cy, rx, ry),
            antialias,
            feather,
        );
        self.apply_selection_mask(mask, mode);
    }

    pub fn select_lasso(
        &mut self,
        vertices: &[[f32; 2]],
        mode: SelectionMode,
        antialias: bool,
        _feather: f32,
    ) {
        if vertices.len() < 3 {
            return;
        }

        let mask =
            crate::mask::rasterize_polygon_r8(self.doc.width, self.doc.height, vertices, antialias);
        self.apply_selection_mask(mask, mode);
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
        // Magic wand operates on full-canvas data — reserve full-canvas undo rect.
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);

        let layer_tex = self.compositor.layer_texture(layer_id).unwrap();
        self.gpu.encode("magic-wand-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                &layer_tex.texture,
                wgpu::TextureFormat::Rgba8Unorm,
                [0, 0, canvas_w, canvas_h],
            );
            self.readbacks.submit(
                request,
                ReadbackContext::MagicWand {
                    was_active,
                    seed_x,
                    seed_y,
                    tolerance,
                    mode,
                },
            );
        });
    }

    pub(crate) fn complete_magic_wand(
        &mut self,
        was_active: bool,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: SelectionMode,
        pixels: Vec<u8>,
    ) {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        let fill_mask =
            flood_fill::flood_fill_rgba(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance);

        self.apply_selection_full(fill_mask, mode, was_active);
    }

    pub fn clear_selection(&mut self) {
        if !self.gpu_selection.active {
            return;
        }
        // Only the pre-op selection region is affected by clear.
        let rect = self
            .gpu_selection
            .pixel_bounds
            .unwrap_or_else(|| self.selection_full_canvas_rect());
        self.save_selection_for_undo(rect);
        let was_active = self.gpu_selection.active;

        self.gpu_selection.clear(&self.gpu.queue);

        self.commit_selection_undo(was_active, rect);
        self.selection_overlay.clear();
        self.push_merged_overlay();
    }

    pub fn select_all(&mut self) {
        // Post-op selection fills the canvas; use full-canvas undo rect.
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.gpu_selection.active;

        let w = self.doc.width;
        let h = self.doc.height;
        let mask = RasterizedMask {
            data: vec![255u8; (w * h) as usize],
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        self.gpu_selection.upload_replace(
            &self.gpu.device,
            &self.gpu.queue,
            &mask,
            self.brush_pipelines.selection_bind_group_layout(),
            &self.paint_pipelines.selection_bind_group_layout,
        );

        self.commit_selection_undo(was_active, rect);
        self.generate_contours_from_mask(&mask);
    }

    pub fn invert_selection(&mut self) {
        if !self.gpu_selection.active {
            return;
        }
        // Invert can produce a selection anywhere on the canvas.
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);
        let was_active = self.gpu_selection.active;

        self.gpu.encode("invert-sel", |encoder| {
            self.selection_pipelines.invert(
                encoder,
                &self.gpu.device,
                &self.gpu.queue,
                &mut self.gpu_selection,
                self.brush_pipelines.selection_bind_group_layout(),
                &self.paint_pipelines.selection_bind_group_layout,
            );
        });
        // Bounds unknown after invert — readback will recompute.
        self.gpu_selection.pixel_bounds = None;
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
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

    /// Apply a tight-bounds rasterized mask (from SDF tools).
    ///
    /// Hot path: rasterize (already done) → upload subregion → done.
    /// Undo save, undo commit, and marching-ants readback are batched into
    /// a single GPU submission so they don't add latency.
    fn apply_selection_mask(&mut self, mask: RasterizedMask, mode: SelectionMode) {
        let was_active = self.gpu_selection.active;
        // Undo rect must cover both the pre-op selection and the new shape —
        // any pixel that might change sits inside this union.
        let rect = self.selection_undo_rect_for_shape([mask.x, mask.y, mask.width, mask.height]);
        self.save_selection_for_undo(rect);

        match mode {
            SelectionMode::Replace => {
                self.gpu_selection.upload_replace(
                    &self.gpu.device,
                    &self.gpu.queue,
                    &mask,
                    self.brush_pipelines.selection_bind_group_layout(),
                    &self.paint_pipelines.selection_bind_group_layout,
                );
                self.generate_contours_from_mask(&mask);
            }
            _ => {
                let cw = self.doc.width;
                let ch = self.doc.height;
                let mut full = vec![0u8; (cw * ch) as usize];
                for y in 0..mask.height {
                    let src = (y * mask.width) as usize;
                    let dst = ((mask.y + y) * cw + mask.x) as usize;
                    full[dst..dst + mask.width as usize]
                        .copy_from_slice(&mask.data[src..src + mask.width as usize]);
                }
                self.apply_combine(&full, mode);
                self.kick_selection_readback();
            }
        }

        self.commit_selection_undo(was_active, rect);
    }

    /// Generate marching ants contours directly from a RasterizedMask (no readback).
    fn generate_contours_from_mask(&mut self, mask: &RasterizedMask) {
        self.selection_overlay.clear();

        if mask.width == 0 || mask.height == 0 {
            self.push_merged_overlay();
            return;
        }

        // contour_segments_r8 expects a full-canvas buffer, but we only have
        // the tight region. Build a minimal padded buffer — just the region
        // plus 1px border for marching squares boundary detection.
        let pad = 1u32;
        let bw = mask.width + 2 * pad;
        let bh = mask.height + 2 * pad;
        let mut buf = vec![0u8; (bw * bh) as usize];
        for y in 0..mask.height {
            let src = (y * mask.width) as usize;
            let dst = ((y + pad) * bw + pad) as usize;
            buf[dst..dst + mask.width as usize]
                .copy_from_slice(&mask.data[src..src + mask.width as usize]);
        }

        let segments = crate::mask::contour_segments_r8(&buf, bw, bh, 127);

        // Offset segments from local coords back to canvas coords.
        let ox = mask.x as f32 - pad as f32;
        let oy = mask.y as f32 - pad as f32;
        for (a, b) in &segments {
            let ca = [a[0] + ox, a[1] + oy];
            let cb = [b[0] + ox, b[1] + oy];
            let mut bg = OverlayPrimitive::new(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, ca, cb);
            bg.color = [0.0, 0.0, 0.0, 1.0];
            bg.thickness = 1.5;
            bg.dash_len = 0.0;
            self.selection_overlay.push(bg);
        }
        for (a, b) in &segments {
            let ca = [a[0] + ox, a[1] + oy];
            let cb = [b[0] + ox, b[1] + oy];
            let mut fg = OverlayPrimitive::new(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, ca, cb);
            fg.color = [1.0, 1.0, 1.0, 1.0];
            fg.thickness = 1.0;
            fg.dash_len = 8.0;
            self.selection_overlay.push(fg);
        }

        self.push_merged_overlay();
    }

    /// Apply a full-canvas R8 buffer (from magic wand, mask-to-selection).
    fn apply_selection_full(
        &mut self,
        shape_pixels: Vec<u8>,
        mode: SelectionMode,
        was_active: bool,
    ) {
        match mode {
            SelectionMode::Replace => {
                self.gpu_selection.upload_replace_full(
                    &self.gpu.device,
                    &self.gpu.queue,
                    &shape_pixels,
                    self.brush_pipelines.selection_bind_group_layout(),
                    &self.paint_pipelines.selection_bind_group_layout,
                );
            }
            _ => {
                self.apply_combine(&shape_pixels, mode);
            }
        }

        // Callers (magic wand, mask-to-selection) reserved a full-canvas save
        // before the async readback — commit must match.
        let rect = self.selection_full_canvas_rect();
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }

    /// Run the GPU combine shader for boolean modes.
    fn apply_combine(&mut self, shape_pixels: &[u8], mode: SelectionMode) {
        let combine_mode = CombineMode::from_selection_mode(&mode);
        self.gpu.encode("sel-combine", |encoder| {
            self.selection_pipelines.combine(
                encoder,
                &self.gpu.device,
                &self.gpu.queue,
                &mut self.gpu_selection,
                shape_pixels,
                combine_mode,
                self.brush_pipelines.selection_bind_group_layout(),
                &self.paint_pipelines.selection_bind_group_layout,
            );
        });
        // Bounds unknown after boolean op.
        self.gpu_selection.pixel_bounds = None;
    }

    // --- Undo helpers ---

    pub(crate) fn save_selection_for_undo(&mut self, rect: [u32; 4]) {
        let texture = self.gpu_selection.texture();
        self.gpu.encode("sel-undo-save", |encoder| {
            self.region_store
                .save_region(encoder, texture, wgpu::TextureFormat::R8Unorm, rect);
        });
    }

    pub(crate) fn commit_selection_undo(&mut self, was_active: bool, rect: [u32; 4]) {
        self.gpu.encode("sel-undo-commit", |encoder| {
            let entry =
                self.region_store
                    .commit_region(encoder, 0, wgpu::TextureFormat::R8Unorm, rect);
            self.undo_stack
                .push(Box::new(SelectionAction::new(was_active, entry)));
        });
    }

    /// Full-canvas undo rect — used when the post-op selection extent isn't
    /// known ahead of time (invert, select-all, magic wand, combine into unknown bounds).
    pub(crate) fn selection_full_canvas_rect(&self) -> [u32; 4] {
        [0, 0, self.doc.width, self.doc.height]
    }

    /// Undo rect that covers both the current (pre-op) selection and a new shape
    /// that will be applied. Save and commit must use the same rect, otherwise
    /// the R8 scratch's stale contents outside the save rect would leak into the
    /// commit buffer and corrupt the selection on undo.
    pub(crate) fn selection_undo_rect_for_shape(&self, shape: [u32; 4]) -> [u32; 4] {
        let cw = self.doc.width;
        let ch = self.doc.height;
        let [sx, sy, sw, sh] = shape;
        let [sx, sy, sw, sh] = [
            sx.min(cw),
            sy.min(ch),
            sw.min(cw - sx.min(cw)),
            sh.min(ch - sy.min(ch)),
        ];
        match self.gpu_selection.pixel_bounds {
            Some([ox, oy, ow, oh]) if sw > 0 && sh > 0 => {
                let x = ox.min(sx);
                let y = oy.min(sy);
                let x_end = (ox + ow).max(sx + sw);
                let y_end = (oy + oh).max(sy + sh);
                [x, y, x_end - x, y_end - y]
            }
            Some(old) => old,
            None => [0, 0, cw, ch],
        }
    }

    /// Kick an async readback for contour extraction (marching ants).
    pub(crate) fn kick_selection_readback(&mut self) {
        let w = self.doc.width;
        let h = self.doc.height;
        let texture = self.gpu_selection.texture();
        self.gpu.encode("sel-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                texture,
                wgpu::TextureFormat::R8Unorm,
                [0, 0, w, h],
            );
            self.readbacks
                .submit(request, ReadbackContext::SelectionReadback);
        });
    }

    // --- Selection overlay ---

    /// Regenerate marching ants from readback data and update CPU cache.
    pub(crate) fn update_selection_overlay_from_readback(&mut self, pixels: Vec<u8>) {
        self.selection_overlay.clear();

        if !self.gpu_selection.active {
            self.push_merged_overlay();
            return;
        }

        // Update pixel_bounds from readback if not already known.
        if self.gpu_selection.pixel_bounds.is_none() {
            self.gpu_selection.pixel_bounds = crate::mask::pixel_bounds_r8(
                &pixels,
                self.gpu_selection.width,
                self.gpu_selection.height,
            );
        }

        // Populate the CPU cache from the readback data.
        self.gpu_selection.cpu_cache = Some(pixels.clone());

        let segments = crate::mask::contour_segments_r8(
            &pixels,
            self.gpu_selection.width,
            self.gpu_selection.height,
            127,
        );
        for (a, b) in &segments {
            let mut bg = OverlayPrimitive::new(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, *a, *b);
            bg.color = [0.0, 0.0, 0.0, 1.0];
            bg.thickness = 1.5;
            bg.dash_len = 0.0;
            self.selection_overlay.push(bg);
        }
        for (a, b) in &segments {
            let mut fg = OverlayPrimitive::new(KIND_DASHED_LINE, FLAG_CANVAS_SPACE, *a, *b);
            fg.color = [1.0, 1.0, 1.0, 1.0];
            fg.thickness = 1.0;
            fg.dash_len = 8.0;
            self.selection_overlay.push(fg);
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

    /// Upload the mask texture sampled by KIND_MASKED_STAMP overlay primitives.
    pub fn set_overlay_mask(&mut self, width: u32, height: u32, rgba: &[u8]) {
        self.compositor
            .set_overlay_mask(&self.gpu.device, &self.gpu.queue, width, height, rgba);
    }

    pub fn clear_overlay_mask(&mut self) {
        self.compositor.clear_overlay_mask();
    }
}
