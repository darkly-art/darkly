//! Destructive color filters — apply a `MaskedFilterPipeline` to a node's
//! pixels in place, respecting an active selection.
//!
//! Mirrors [`layer_flip`](super::super::layer_flip)'s structure: the region machinery
//! is node-generic (layers and masks both live in `node_textures` keyed by
//! `LayerId`, RGBA8 vs R8 driven by format), so a single `apply_filter`
//! inverts a raster layer or a mask with no per-kind branching. With an active
//! selection only the selected region changes — clipped to the selection
//! *shape* via the uploaded mask; with none, the whole node is filtered. The
//! pixel extent never changes, so undo is a single [`GpuRegionAction`].

use darkly_macros::handlers;

use super::super::rendering::commit_undo_region;
use super::super::{DarklyEngine, PendingFilter};
use crate::coord::WindowRect;
use crate::engine::protocol::{params_from_json, RawParams};
use crate::engine::types::{ParamInfo, VeilTypeInfo};
use crate::gpu::params::ParamValue;
use crate::layer::LayerId;
use crate::undo::GpuRegionAction;

#[handlers]
impl DarklyEngine {
    /// All registered filter types (id + display name + param schema), as the
    /// same `VeilTypeInfo` shape veils/voids use. Parameter-free filters (invert)
    /// carry an empty `params`; parametric ones (curves) carry their schema.
    /// Drives both the frontend's dynamic Colors-menu action registration and
    /// the filter-layer properties panel.
    #[handler]
    pub fn filter_types(&self) -> Vec<VeilTypeInfo> {
        let registry = self.compositor.filter_pipeline_registry();
        registry
            .types()
            .into_iter()
            .map(|(type_id, display_name)| VeilTypeInfo {
                type_id,
                display_name,
                params: registry
                    .params(type_id)
                    .iter()
                    .map(|d| ParamInfo::from_def(d, None))
                    .collect(),
            })
            .collect()
    }

    /// Wire entry for `apply_filter` — coerces `params` against the filter
    /// type's schema (defaults fill any omitted values), then
    /// [`Self::apply_filter_typed`]. Parameter-free filters (invert) carry an
    /// empty `params`; parametric ones (curves/levels/hsv) carry the values the
    /// destructive modal authored.
    #[handler]
    pub fn apply_filter(
        &mut self,
        node_id: LayerId,
        filter_type: String,
        params: RawParams,
    ) -> bool {
        let pv = params_from_json(&params.0, self.filter_param_defs(&filter_type));
        self.apply_filter_typed(node_id, &filter_type, pv)
    }

    /// Apply a destructive filter (by registered `filter_type` id) to the given
    /// node (raster layer or mask filter) with typed `params`. Returns `false`
    /// if the node isn't editable, has no texture, the type is unknown, or the
    /// filter was deferred waiting on the selection cache. Parametric filters
    /// bake `params` into a throwaway cache inside `filter_node_region`, exactly
    /// as the filter-layer compose path does.
    pub fn apply_filter_typed(
        &mut self,
        node_id: LayerId,
        filter_type: &str,
        params: Vec<ParamValue>,
    ) -> bool {
        if !self.doc.is_node_editable(node_id) {
            return false;
        }
        if !self.compositor.filter_pipeline_registry().has(filter_type) {
            return false;
        }
        self.auto_commit_floating();
        if self.doc.layer(node_id).is_none() && self.doc.find_filter(node_id).is_none() {
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
                            self.pending_filter = Some(PendingFilter {
                                node_id,
                                filter_type: filter_type.to_string(),
                                params,
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
            .compositor
            .filter_pipeline_registry_mut()
            .pipeline(filter_type, &self.gpu.device)
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
        let snap = self.gpu.encode_ret("filter-save", |enc| {
            self.region_scratch
                .save_region(&self.gpu.device, enc, &frame, format, region)
        });
        let entry = commit_undo_region(
            &self.gpu,
            &self.region_scratch,
            &mut self.readbacks,
            "filter-commit",
            node_id,
            &frame,
            &snap,
            region,
        );

        // Filter the region in place, masked to the selection shape when present.
        let mask_view = mask_tex
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        self.gpu.encode("apply-filter", |enc| {
            self.compositor.filter_node_region(
                &self.gpu.device,
                &self.gpu.queue,
                enc,
                node_id,
                region,
                mask_view.as_ref(),
                pipeline.as_ref(),
                &params,
            );
        });

        self.push_undo(Box::new(GpuRegionAction::new(entry)));
        self.compositor.mark_dirty();
        true
    }
}
