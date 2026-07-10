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
use super::super::{DarklyEngine, FilterPreview, PendingFilter};
use crate::coord::{CanvasRect, WindowRect};
use crate::engine::protocol::{params_from_json, RawParams};
use crate::engine::types::{ParamInfo, VeilTypeInfo};
use crate::gpu::params::ParamValue;
use crate::layer::LayerId;
use crate::undo::GpuRegionAction;

/// The destructive region a node filter should touch, plus its selection mask.
/// Shared by the one-shot [`apply_filter_typed`](DarklyEngine::apply_filter_typed)
/// and the live [`preview_filter`](DarklyEngine::preview_filter) paths so both
/// clip to the selection identically.
pub(crate) enum FilterRegion {
    /// Filter this region, masked to the selection shape when `Some`.
    Ready(CanvasRect, Option<wgpu::Texture>),
    /// The selection resolved but is degenerate — nothing to filter.
    Empty,
    /// The selection bbox isn't cached yet; the caller kicks a readback.
    NeedsSelection,
}

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
        let format = match self.compositor.node_texture(node_id) {
            Some(t) => t.format(),
            None => return false,
        };

        // Region to filter + (for a selection) the mask cropped to it.
        let (region, mask_tex) = match self.resolve_filter_region(node_id) {
            FilterRegion::Ready(region, mask_tex) => (region, mask_tex),
            FilterRegion::Empty => return false,
            FilterRegion::NeedsSelection => {
                self.pending_filter = Some(PendingFilter {
                    node_id,
                    filter_type: filter_type.to_string(),
                    params,
                });
                self.kick_selection_readback();
                return false;
            }
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

    /// Wire entry for `preview_filter` — coerces `params`, then
    /// [`Self::preview_filter_typed`]. Drives the destructive modal's live
    /// preview: the effect is shown on the canvas non-destructively until the
    /// user commits or cancels.
    #[handler]
    pub fn preview_filter(
        &mut self,
        node_id: LayerId,
        filter_type: String,
        params: RawParams,
    ) -> bool {
        let pv = params_from_json(&params.0, self.filter_param_defs(&filter_type));
        self.preview_filter_typed(node_id, &filter_type, pv)
    }

    /// Show `params` applied to `node_id` live, without an undo entry. On the
    /// first call for a (node, type) it snapshots the affected region (clipped to
    /// the selection); every call restores that pristine snapshot and re-filters
    /// with the new params, so dragging a slider re-previews cleanly. Switching
    /// node/type restores the previous preview first. Commit / cancel go through
    /// [`Self::commit_filter_preview`] / [`Self::cancel_filter_preview`].
    pub fn preview_filter_typed(
        &mut self,
        node_id: LayerId,
        filter_type: &str,
        params: Vec<ParamValue>,
    ) -> bool {
        if !self.compositor.filter_pipeline_registry().has(filter_type) {
            return false;
        }
        // A preview for a different node/type must be undone before starting a new one.
        if self
            .filter_preview
            .as_ref()
            .is_some_and(|p| p.node_id != node_id || p.filter_type != filter_type)
        {
            self.cancel_filter_preview();
        }

        // Begin a session (snapshot the pristine region) if none is active.
        if self.filter_preview.is_none() {
            self.auto_commit_floating();
            let (region, mask) = match self.resolve_filter_region(node_id) {
                FilterRegion::Ready(r, m) => (r, m),
                FilterRegion::Empty => return false,
                FilterRegion::NeedsSelection => {
                    // The commit path is authoritative and will clip correctly;
                    // just kick the readback so a subsequent edit can preview.
                    self.kick_selection_readback();
                    return false;
                }
            };
            let (snapshot, region) = match self.compositor.snapshot_node_region(
                &self.gpu.device,
                &self.gpu.queue,
                node_id,
                region,
            ) {
                Some(s) => s,
                None => return false,
            };
            self.filter_preview = Some(FilterPreview {
                node_id,
                filter_type: filter_type.to_string(),
                region,
                snapshot,
                mask,
            });
        }

        // Restore the pristine pixels, then filter them with the new params.
        let Some(preview) = self.filter_preview.take() else {
            return false;
        };
        self.compositor.restore_node_region(
            &self.gpu.device,
            &self.gpu.queue,
            node_id,
            preview.region,
            &preview.snapshot,
        );
        let pipeline = match self
            .compositor
            .filter_pipeline_registry_mut()
            .pipeline(filter_type, &self.gpu.device)
        {
            Some(p) => p,
            None => {
                self.filter_preview = Some(preview);
                return false;
            }
        };
        let mask_view = preview
            .mask
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        let region = preview.region;
        self.gpu.encode("preview-filter", |enc| {
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
        self.filter_preview = Some(preview);
        self.compositor.mark_dirty();
        true
    }

    /// Discard a live preview, restoring the node's pristine pixels. A no-op
    /// when no preview is active (e.g. the user never edited a param).
    #[handler]
    pub fn cancel_filter_preview(&mut self) {
        if let Some(preview) = self.filter_preview.take() {
            self.compositor.restore_node_region(
                &self.gpu.device,
                &self.gpu.queue,
                preview.node_id,
                preview.region,
                &preview.snapshot,
            );
        }
    }

    /// Commit a destructive filter from the modal: restore the pristine pixels
    /// (so the authoritative apply snapshots the true "before" and clips the
    /// selection itself), then [`Self::apply_filter_typed`] once — one undo
    /// entry. Works whether or not a live preview was ever established.
    #[handler]
    pub fn commit_filter_preview(
        &mut self,
        node_id: LayerId,
        filter_type: String,
        params: RawParams,
    ) -> bool {
        let pv = params_from_json(&params.0, self.filter_param_defs(&filter_type));
        self.commit_filter_preview_typed(node_id, &filter_type, pv)
    }

    /// Restore the pristine preview pixels then [`Self::apply_filter_typed`] once
    /// with typed `params`. The typed core behind [`Self::commit_filter_preview`].
    pub fn commit_filter_preview_typed(
        &mut self,
        node_id: LayerId,
        filter_type: &str,
        params: Vec<ParamValue>,
    ) -> bool {
        if let Some(preview) = self.filter_preview.take() {
            self.compositor.restore_node_region(
                &self.gpu.device,
                &self.gpu.queue,
                preview.node_id,
                preview.region,
                &preview.snapshot,
            );
        }
        self.apply_filter_typed(node_id, filter_type, params)
    }

    /// Resolve the region a node filter should touch (canvas coords), plus the
    /// cropped R8 selection mask when a selection is active. With no selection
    /// the whole node extent is returned unmasked. `NeedsSelection` means the
    /// selection bbox readback hasn't landed — the caller decides whether to
    /// defer (destructive apply) or fall back (live preview).
    pub(crate) fn resolve_filter_region(&mut self, node_id: LayerId) -> FilterRegion {
        let node_extent = match self.compositor.node_texture(node_id) {
            Some(t) => t.canvas_extent(),
            None => return FilterRegion::Empty,
        };
        if !self.has_selection() {
            return FilterRegion::Ready(node_extent, None);
        }
        // Selection bbox (window-local) — recomputed from the cpu cache if the
        // readback hasn't populated bounds yet, else NeedsSelection.
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
                    None => return FilterRegion::NeedsSelection,
                }
            }
        };
        if bounds.width == 0 || bounds.height == 0 {
            return FilterRegion::Empty;
        }
        // Window-local bbox → plane, clipped to the node extent.
        let region_plane = bounds.to_canvas(self.doc.canvas_origin);
        let region = match node_extent.intersect(region_plane) {
            Some(r) if r.width > 0 && r.height > 0 => r,
            _ => return FilterRegion::Empty,
        };
        let win_origin = (
            region.origin.x - self.doc.canvas_origin.x,
            region.origin.y - self.doc.canvas_origin.y,
        );
        let mask_tex =
            self.upload_cropped_selection_texture(win_origin, region.width, region.height);
        FilterRegion::Ready(region, mask_tex)
    }
}
