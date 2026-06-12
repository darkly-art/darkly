//! Canvas resize & crop-to-selection.
//!
//! Both features move the **canvas window** within the fixed canvas/plane.
//! Content, layer extents, and the undo stack stay put in the plane; only the
//! window moves, so undo is exact and document-only. See the
//! `Document::canvas_origin` doc for the coordinate model. (Content-*scaling*
//! resize is a planned follow-up — it needs lossy per-node pixel-snapshot
//! undo, distinct from this document-only path.)

use super::DarklyEngine;
use crate::coord::{CanvasPoint, CanvasRect};
use crate::undo::CanvasResizeAction;

/// Largest canvas dimension we allow, matching wgpu's common
/// `max_texture_dimension_2d`. A content-scaling resize that would push any
/// layer extent past this is refused rather than silently clamped.
pub const MAX_CANVAS_DIM: u32 = 8192;

impl DarklyEngine {
    /// New canvas-window rect for an anchored resize (no content scaling).
    ///
    /// `(ax, ay)` are the 9-point anchor, each in `{0.0, 0.5, 1.0}`: the
    /// fraction of the width/height delta removed from the top/left edge. Grows
    /// and shrinks both work (the delta is signed). Pure — unit-tested.
    pub fn resize_anchor_rect(
        old_origin: CanvasPoint,
        old_w: u32,
        old_h: u32,
        new_w: u32,
        new_h: u32,
        ax: f32,
        ay: f32,
    ) -> CanvasRect {
        let dx = ((old_w as f32 - new_w as f32) * ax).round() as i32;
        let dy = ((old_h as f32 - new_h as f32) * ay).round() as i32;
        CanvasRect::new(
            CanvasPoint::new(old_origin.x + dx, old_origin.y + dy),
            new_w,
            new_h,
        )
    }

    /// Resize / move the canvas window.
    ///
    /// The canvas window as a plane-space rect — `(canvas_origin, width,
    /// height)`. Single public read of the document's canvas window (the WASM
    /// `canvas_rect()` query and tests read through this).
    pub fn canvas_rect(&self) -> CanvasRect {
        self.doc.canvas_rect()
    }

    /// Anchored resize from the resize modal: resolve the new window rect from
    /// the 9-point anchor `(ax, ay)` (each in `{0.0, 0.5, 1.0}`) and apply it.
    /// No content scaling — see [`Self::resize_canvas`].
    pub fn resize_canvas_anchored(&mut self, new_w: u32, new_h: u32, ax: f32, ay: f32) {
        let rect = Self::resize_anchor_rect(
            self.doc.canvas_origin,
            self.doc.width,
            self.doc.height,
            new_w,
            new_h,
            ax,
            ay,
        );
        self.resize_canvas(rect);
    }

    /// Moves/crops the canvas window: content, layer extents, and the undo
    /// stack stay put in the plane — only the window moves, so undo is exact
    /// and document-only. No-ops on a zero/over-limit target or a no-op move.
    ///
    /// (Content-*scaling* resize — resampling every layer about the window
    /// origin — is a planned follow-up; it needs lossy per-node pixel-snapshot
    /// undo, distinct from this document-only path.)
    pub fn resize_canvas(&mut self, new_rect: CanvasRect) {
        self.auto_commit_floating();
        if new_rect.width == 0
            || new_rect.height == 0
            || new_rect.width > MAX_CANVAS_DIM
            || new_rect.height > MAX_CANVAS_DIM
        {
            return;
        }
        let old = (self.doc.canvas_origin, self.doc.width, self.doc.height);
        let new = (new_rect.origin, new_rect.width, new_rect.height);
        if old == new {
            return;
        }
        self.doc.canvas_origin = new_rect.origin;
        self.doc.width = new_rect.width;
        self.doc.height = new_rect.height;
        self.push_undo(Box::new(CanvasResizeAction::new(old, new)));
        self.apply_canvas_rect_to_compositor();
        self.compositor.mark_dirty();
    }

    /// Crop the canvas to the active selection's plane bounds. No-op if there
    /// is no active selection (or its bounds are still empty/unknown).
    pub fn crop_to_selection(&mut self) {
        if !self.has_selection() {
            return;
        }
        // Selection pixel bounds are *window-local* (see CLAUDE.md selection
        // notes); fall back to recomputing them from the CPU cache when the
        // async readback hasn't landed yet.
        let local = match self.selection_pixel_bounds().filter(|b| !b.is_empty()) {
            Some(b) => b,
            None => match self.selection_cpu_cache().and_then(|data| {
                crate::mask::pixel_bounds_r8(data, self.doc.width, self.doc.height)
                    .map(|[x, y, w, h]| CanvasRect::from_xywh(x as i32, y as i32, w, h))
            }) {
                Some(b) => b,
                None => return,
            },
        };
        // Lift window-local bounds into the plane: the crop window is a plane
        // rect anchored at the current window origin.
        let o = self.doc.canvas_origin;
        let plane = CanvasRect::from_xywh(
            o.x + local.x0(),
            o.y + local.y0(),
            local.width,
            local.height,
        );
        self.resize_canvas(plane);
    }

    /// Push the document's canvas rect onto the compositor — recreates the
    /// window-sized GPU resources and re-realizes the selection mask. Shared by
    /// the resize op and the undo-reconcile hook.
    pub(crate) fn apply_canvas_rect_to_compositor(&mut self) {
        let rect = self.doc.canvas_rect();
        self.compositor.set_canvas_rect(
            &self.gpu.device,
            &self.gpu.queue,
            rect.origin,
            rect.width,
            rect.height,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(ax: f32, ay: f32, new_w: u32, new_h: u32) -> CanvasRect {
        // From a 100×100 canvas at the plane origin.
        DarklyEngine::resize_anchor_rect(CanvasPoint::new(0, 0), 100, 100, new_w, new_h, ax, ay)
    }

    #[test]
    fn anchor_top_left_keeps_origin_when_growing() {
        // (0,0) anchor removes none of the delta from the top/left.
        assert_eq!(
            anchor(0.0, 0.0, 140, 160),
            CanvasRect::from_xywh(0, 0, 140, 160)
        );
    }

    #[test]
    fn anchor_bottom_right_grows_up_and_left() {
        // (1,1) anchor removes the whole delta from the top/left, so the window
        // origin moves to -(delta).
        assert_eq!(
            anchor(1.0, 1.0, 140, 160),
            CanvasRect::from_xywh(-40, -60, 140, 160)
        );
    }

    #[test]
    fn anchor_center_splits_the_delta() {
        // (0.5,0.5) centers: half the delta off each edge.
        assert_eq!(
            anchor(0.5, 0.5, 140, 160),
            CanvasRect::from_xywh(-20, -30, 140, 160)
        );
    }

    #[test]
    fn anchor_center_shrink_pulls_inward() {
        // Shrinking with a center anchor moves the origin inward (positive).
        assert_eq!(
            anchor(0.5, 0.5, 60, 40),
            CanvasRect::from_xywh(20, 30, 60, 40)
        );
    }

    #[test]
    fn anchor_top_left_shrink_keeps_origin() {
        assert_eq!(
            anchor(0.0, 0.0, 60, 40),
            CanvasRect::from_xywh(0, 0, 60, 40)
        );
    }

    #[test]
    fn anchor_respects_existing_origin() {
        // A non-zero starting origin offsets the result by the same amount.
        let r =
            DarklyEngine::resize_anchor_rect(CanvasPoint::new(10, 5), 100, 100, 140, 160, 0.5, 0.5);
        assert_eq!(r, CanvasRect::from_xywh(-10, -25, 140, 160));
    }
}
