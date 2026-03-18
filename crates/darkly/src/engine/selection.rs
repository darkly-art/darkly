//! Selection operations — rect, ellipse, lasso, magic wand, clear, invert.

use super::{DarklyEngine, ReadbackContext};
use crate::document::SelectionMode;
use crate::gpu::flood_fill;
use crate::gpu::overlay::{OverlayPrimitive, KIND_DASHED_LINE, FLAG_CANVAS_SPACE};
use crate::gpu::readback;
use crate::tile::AlphaMask;
use crate::undo::SelectionAction;

impl DarklyEngine {
    pub fn select_rect(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::rect_select::rasterize(x, y, w, h, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_ellipse(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::ellipse_select::rasterize(x, y, w, h, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_lasso(
        &mut self,
        vertices: &[[f32; 2]],
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::lasso_select::rasterize(vertices, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_magic_wand(
        &mut self,
        layer_id: u64,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: SelectionMode,
    ) {
        let layer_tex = match self.compositor.layer_texture(layer_id) {
            Some(t) => t,
            _ => return,
        };
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        let old_sel = self.doc.selection.clone();

        // Async GPU readback of layer texture.
        self.gpu.encode("magic-wand-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device, encoder, &layer_tex.texture,
                wgpu::TextureFormat::Rgba8Unorm, [0, 0, canvas_w, canvas_h],
            );
            self.readbacks.submit(request, ReadbackContext::MagicWand {
                old_sel, seed_x, seed_y, tolerance, mode,
            });
        });
    }

    /// Complete magic wand after async readback.
    pub(crate) fn complete_magic_wand(
        &mut self, old_sel: Option<AlphaMask>, seed_x: i32, seed_y: i32,
        tolerance: u8, mode: SelectionMode, pixels: Vec<u8>,
    ) {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // CPU scanline flood fill on the flat pixel data.
        let fill_mask = flood_fill::flood_fill_rgba(
            &pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance,
        );

        // Convert R8 fill result to AlphaMask.
        let mask = AlphaMask::from_r8(&fill_mask, canvas_w, canvas_h);

        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn clear_selection(&mut self) {
        if self.doc.selection.is_none() {
            return;
        }
        let old_sel = self.doc.selection.clone();
        self.doc.selection = None;
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_all(&mut self) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::rect_select::rasterize(
            0.0, 0.0, self.doc.width as f32, self.doc.height as f32, false, 0.0,
        );
        self.doc.selection = Some(mask);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn invert_selection(&mut self) {
        let old_sel = self.doc.selection.clone();
        if let Some(sel) = &mut self.doc.selection {
            sel.invert(self.doc.width, self.doc.height);
        }
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn clear_selection_contents(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        if self.doc.selection.is_none() {
            return;
        }
        self.gpu_clear_selection(layer_id);
    }

    pub fn has_selection(&self) -> bool {
        self.doc.selection.is_some()
    }

    // --- Selection overlay ---

    /// Regenerate marching ants overlay from the current selection.
    pub(crate) fn update_selection_overlay(&mut self) {
        self.selection_overlay.clear();

        if let Some(sel) = &self.doc.selection {
            let segments = sel.contour_segments(0.5);
            for (a, b) in &segments {
                // Black background line (slightly thicker, solid)
                let mut bg = OverlayPrimitive::new(
                    KIND_DASHED_LINE,
                    FLAG_CANVAS_SPACE,
                    *a, *b,
                );
                bg.color = [0.0, 0.0, 0.0, 1.0];
                bg.thickness = 1.5;
                bg.dash_len = 0.0; // solid
                self.selection_overlay.push(bg);
            }
            for (a, b) in &segments {
                // White foreground dashes
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
