//! Layer / selection flip (horizontal / vertical) — exact mirror.
//!
//! Mirrors the active node's pixels in place about a region centre, via the
//! same exact ortho primitive as canvas flip/rotate (`gpu/ortho_transform`).
//! With an active selection only the selected region flips — about the
//! selection's bounding-box centre, masked to the selection *shape* (Krita's
//! `KisMirrorProcessingVisitor`); with none, the whole layer flips about its
//! own extent centre. Horizontal/vertical only (no layer rotate), so the node
//! extent never changes and undo is a single [`GpuRegionAction`].

use std::rc::Rc;

use serde_json::json;

use super::host::cell::EngineCell;
use super::host::combinators::ensure_selection_cache_warm;
use super::protocol::Response;
use super::rendering::commit_undo_region;
use super::DarklyEngine;
use crate::coord::WindowRect;
use crate::gpu::ortho_transform::OrthoXform;
use crate::layer::LayerId;
use crate::undo::GpuRegionAction;

/// Run a flip as a deferred task: warm the selection cache if cold (the flip
/// pivot is the selection bbox centre, read from that cache), then perform the
/// flip and resolve the originating request with `{ ok }`.
async fn run_flip(cell: Rc<EngineCell>, node_id: LayerId, xform: OrthoXform, request: u64) {
    ensure_selection_cache_warm(&cell).await;
    let ok = cell.with_async(|e| e.flip_node(node_id, xform)).await;
    cell.with_async(|e| e.resolve_request(request, Response::json(json!({ "ok": ok }))))
        .await;
}

impl DarklyEngine {
    /// Spawn the flip task — see [`run_flip`]. The originating request resolves
    /// with `{ ok }` once the flip lands (immediately, or the next frame if the
    /// selection cache had to be warmed first).
    pub fn spawn_flip(&mut self, node_id: LayerId, xform: OrthoXform) {
        let request = self.current_request();
        let cell = self.self_cell();
        self.spawn(Some(request), run_flip(cell, node_id, xform, request));
    }

    /// Flip the given node (raster layer or mask modifier) horizontally or
    /// vertically. Returns `false` if the node isn't editable, has no texture,
    /// or the selection cache is cold (the [`run_flip`] task warms it and
    /// retries, so callers going through the task never see that last case).
    pub fn flip_node(&mut self, node_id: LayerId, xform: OrthoXform) -> bool {
        debug_assert!(matches!(xform, OrthoXform::FlipH | OrthoXform::FlipV));
        if !self.doc.is_node_editable(node_id) {
            return false;
        }
        self.auto_commit_floating();
        if self.doc.layer(node_id).is_none() && self.doc.find_modifier(node_id).is_none() {
            return false;
        }
        let (node_extent, format) = match self.compositor.node_texture(node_id) {
            Some(t) => (t.canvas_extent(), t.format()),
            None => return false,
        };

        // Region to mirror + (for a selection) the mask cropped to it.
        let (region, mask_tex) = if self.has_selection() {
            // Selection bbox (window-local) — recomputed from the cpu cache if
            // the readback hasn't populated bounds yet, else deferred.
            let bounds = match self.selection_pixel_bounds() {
                Some(b) => b,
                None => {
                    let recomputed = self.selection_cpu_cache().and_then(|d| {
                        crate::mask::pixel_bounds_r8(d, self.doc.width, self.doc.height)
                            .map(|[x, y, w, h]| WindowRect::from_xywh(x as i32, y as i32, w, h))
                    });
                    match recomputed {
                        Some(b) => {
                            self.set_selection_pixel_bounds(Some(b));
                            b
                        }
                        // Cache cold — the `run_flip` task warms it before
                        // calling, so this only short-circuits a direct caller.
                        None => return false,
                    }
                }
            };
            if bounds.width == 0 || bounds.height == 0 {
                return false;
            }
            // Window-local bbox → plane, clipped to the node extent (keeps the
            // mirror in-place; the pivot is the clipped region's centre).
            let region_plane = bounds.to_canvas(self.doc.canvas_origin);
            let region = match node_extent.intersect(region_plane) {
                Some(r) if r.width > 0 && r.height > 0 => r,
                _ => return false,
            };
            let win_origin = (
                region.origin.x - self.doc.canvas_origin.x,
                region.origin.y - self.doc.canvas_origin.y,
            );
            let mask_tex =
                self.upload_cropped_selection_texture(win_origin, region.width, region.height);
            (region, mask_tex)
        } else {
            (node_extent, None)
        };

        // Snapshot the affected region for a single-step undo.
        let frame = self
            .compositor
            .node_texture(node_id)
            .expect("checked above")
            .canvas_frame();
        let snap = self.gpu.encode_ret("flip-node-save", |enc| {
            self.region_scratch
                .save_region(&self.gpu.device, enc, &frame, format, region)
        });
        let entry = commit_undo_region(
            &self.gpu,
            &self.region_scratch,
            &mut self.readbacks,
            "flip-node-commit",
            node_id,
            &frame,
            &snap,
            region,
        );

        // Permute the region in place, masked to the selection shape when present.
        let mask_view = mask_tex
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        self.gpu.encode("flip-node", |enc| {
            self.compositor.flip_node_region(
                &self.gpu.device,
                &self.gpu.queue,
                enc,
                node_id,
                region,
                xform,
                mask_view.as_ref(),
            );
        });

        self.push_undo(Box::new(GpuRegionAction::new(entry)));
        self.compositor.mark_dirty();
        true
    }
}
