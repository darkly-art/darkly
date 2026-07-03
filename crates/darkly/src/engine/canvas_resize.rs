//! Canvas resize & crop-to-selection.
//!
//! Both features move the **canvas window** within the fixed canvas/plane.
//! Content, layer extents, and the undo stack stay put in the plane; only the
//! window moves, so undo is exact and document-only. See the
//! `Document::canvas_origin` doc for the coordinate model. (Content-*scaling*
//! resize is a planned follow-up — it needs lossy per-node pixel-snapshot
//! undo, distinct from this document-only path.)

use darkly_macros::handlers;

use super::DarklyEngine;
use crate::coord::{CanvasRect, WindowRect};
use crate::undo::CanvasResizeAction;

/// Largest canvas dimension we allow, matching wgpu's common
/// `max_texture_dimension_2d`. A content-scaling resize that would push any
/// layer extent past this is refused rather than silently clamped.
pub const MAX_CANVAS_DIM: u32 = 8192;

#[handlers]
impl DarklyEngine {
    /// Resize / move the canvas window.
    ///
    /// The canvas window as a plane-space rect — `(canvas_origin, width,
    /// height)`. Single public read of the document's canvas window (the WASM
    /// `canvas_rect()` query and tests read through this).
    pub fn canvas_rect(&self) -> CanvasRect {
        self.doc.canvas_rect()
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
    #[handler]
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
                    .map(|[x, y, w, h]| WindowRect::from_xywh(x as i32, y as i32, w, h))
            }) {
                Some(b) => b,
                None => return,
            },
        };
        // Lift window-local bounds into the plane: the crop window is a plane
        // rect anchored at the current window origin.
        self.resize_canvas(local.to_canvas(self.doc.canvas_origin));
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
        // The present view matrix bakes in the canvas dimensions (both the
        // sampling-normalization and the canvas center). `set_canvas_rect`
        // reallocated the composite cache at the new size but left the cached
        // matrix encoding the old dims — re-derive it from the unchanged view
        // params + the new dims so the next present isn't stretched/offset.
        self.rebuild_view_transform();

        // `set_canvas_rect` re-realized the selection mask at the moved window
        // (preserving its plane pixels), but the doc-side window-local caches
        // are now stale: `pixel_bounds` / `cpu_cache` were measured against the
        // OLD window, and the marching ants were emitted for it. Drop them and
        // kick a readback to repopulate against the new window — which also
        // regenerates the ants. Async, so a transform/copy issued before it
        // lands defers through the existing pending-op mechanism.
        if self.has_selection() {
            self.set_selection_pixel_bounds(None);
            self.invalidate_selection_cpu_cache();
            self.kick_selection_readback();
        }
    }
}
