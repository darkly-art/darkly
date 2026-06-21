//! Destructive color adjustments — apply a `MaskedFilterPipeline` to a node's
//! pixels in place, respecting an active selection.
//!
//! Mirrors [`layer_flip`](super::layer_flip)'s structure: the region machinery
//! is node-generic (layers and masks both live in `node_textures` keyed by
//! `LayerId`, RGBA8 vs R8 driven by format), so a single `apply_adjustment`
//! inverts a raster layer or a mask with no per-kind branching. With an active
//! selection only the selected region changes — clipped to the selection
//! *shape* via the uploaded mask; with none, the whole node is filtered. The
//! pixel extent never changes, so undo is a single [`GpuRegionAction`].

use super::rendering::commit_undo_region;
use super::{DarklyEngine, PendingAdjustment};
use crate::coord::WindowRect;
use crate::engine::types::VeilTypeInfo;
use crate::layer::LayerId;
use crate::undo::GpuRegionAction;

impl DarklyEngine {
    /// All registered adjustment types (id + display name), as the same
    /// `VeilTypeInfo` shape veils/voids use — adjustments carry no params, so
    /// the list is empty there. Drives the frontend's dynamic Colors-menu
    /// action registration.
    pub fn adjustment_types(&self) -> Vec<VeilTypeInfo> {
        self.adjustment_registry
            .types()
            .into_iter()
            .map(|(type_id, display_name)| VeilTypeInfo {
                type_id,
                display_name,
                params: Vec::new(),
            })
            .collect()
    }

    /// Apply a destructive adjustment (by registered `adjustment_type` id) to
    /// the given node (raster layer or mask modifier). Returns `false` if the
    /// node isn't editable, has no texture, the type is unknown, or the
    /// adjustment was deferred waiting on the selection cache.
    pub fn apply_adjustment(&mut self, node_id: LayerId, adjustment_type: &str) -> bool {
        if !self.doc.is_node_editable(node_id) {
            return false;
        }
        if !self.adjustment_registry.has(adjustment_type) {
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

        // Region to filter + (for a selection) the mask cropped to it.
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
                        None => {
                            self.pending_adjustment = Some(PendingAdjustment {
                                node_id,
                                adjustment_type: adjustment_type.to_string(),
                            });
                            self.kick_selection_readback();
                            return false;
                        }
                    }
                }
            };
            if bounds.width == 0 || bounds.height == 0 {
                return false;
            }
            // Window-local bbox → plane, clipped to the node extent.
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

        // Resolve the (lazily-built, shared) pipeline before touching undo so a
        // missing type fails before we snapshot. `has()` above guarantees Some.
        let pipeline = match self
            .adjustment_registry
            .pipeline(adjustment_type, &self.gpu.device)
        {
            Some(p) => p,
            None => return false,
        };

        // Snapshot the affected region for a single-step undo.
        let frame = self
            .compositor
            .node_texture(node_id)
            .expect("checked above")
            .canvas_frame();
        let snap = self.gpu.encode_ret("adjustment-save", |enc| {
            self.region_scratch
                .save_region(&self.gpu.device, enc, &frame, format, region)
        });
        let entry = commit_undo_region(
            &self.gpu,
            &self.region_scratch,
            &mut self.readbacks,
            "adjustment-commit",
            node_id,
            &frame,
            &snap,
            region,
        );

        // Filter the region in place, masked to the selection shape when present.
        let mask_view = mask_tex
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        self.gpu.encode("apply-adjustment", |enc| {
            self.compositor.filter_node_region(
                &self.gpu.device,
                &self.gpu.queue,
                enc,
                node_id,
                region,
                mask_view.as_ref(),
                &pipeline,
            );
        });

        self.push_undo(Box::new(GpuRegionAction::new(entry)));
        self.compositor.mark_dirty();
        true
    }
}
