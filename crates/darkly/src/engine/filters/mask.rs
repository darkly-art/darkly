//! Engine-level operations on mask filters.
//!
//! Replaces the old `engine/masks.rs`. Add/remove/apply now go through the
//! generic `Document::add_mask_filter` / `Document::remove_filter` helpers
//! and the unified compositor node-texture pool. The "active node = paint
//! target" rule means there's no `editing_mask_layer` redirect — the active
//! node id directly identifies where strokes are routed.

use super::super::rendering::commit_undo_region;
use super::super::DarklyEngine;
use crate::layer::LayerId;
use crate::undo::{
    CompoundAction, FilterAddAction, FilterRemoveAction, GpuRegionAction, UndoAction,
};

impl DarklyEngine {
    /// Attach a mask filter to a host layer or group, allocating its GPU
    /// texture in the unified node-texture pool. If a selection is active,
    /// the mask is seeded from the selection (one-click "selection → mask").
    pub fn add_mask(&mut self, host_id: LayerId) {
        if !self.doc.is_node_editable(host_id) {
            return;
        }
        // UI invariant: at most one mask per host. The model supports N; we
        // refuse here so that `add_mask_filter` doesn't silently create a
        // second one.
        // host unknown → bail (true keeps the existing semantics).
        if self.doc.find_node(host_id).is_none() {
            return;
        }
        if self.doc.has_mask(host_id) {
            return;
        }

        let mod_id = match self.doc.add_mask_filter(host_id) {
            Some(id) => id,
            None => return,
        };

        let bounds = match self.doc.find_filter(mod_id).and_then(|m| m.pixels()) {
            Some(buf) => buf.bounds,
            None => return,
        };
        self.compositor.ensure_node_texture(
            &self.gpu.device,
            &self.gpu.queue,
            mod_id,
            wgpu::TextureFormat::R8Unorm,
            bounds,
        );

        // Per-host snapshot+lerp resource for the passthrough-group-with-mask
        // path. Idempotent across both raster and group hosts; only the
        // group composite path consumes it, but the engine doesn't need to
        // branch — the compositor reads it lazily.
        self.compositor
            .ensure_passthrough_mask_state(&self.gpu.device, host_id);

        // If a selection is active, seed the mask pixels from the selection.
        // Grow the mask to the union of its bounds and the selection's plane
        // bounds first, so the whole selection is represented (the mask may
        // default to a sub-canvas host's bounds, smaller than the selection).
        if self.has_selection() {
            if let Some(src_id) = self.selection_modifier_id() {
                self.grow_filter(mod_id, self.selection_seed_bounds());
                self.clone_filter_pixels(src_id, mod_id);
            }
        }

        // ensure_node_texture (fresh allocation) and clone_filter_pixels
        // (selection-seeded copy, when present) already mark the filter
        // dirty per the write-site invariant.
        self.compositor.mark_dirty();

        self.push_undo(Box::new(FilterAddAction::new(mod_id, host_id)));
    }

    /// Remove the mask filter on a host layer or group.
    pub fn remove_mask(&mut self, host_id: LayerId) {
        if !self.doc.is_node_editable(host_id) {
            return;
        }
        let mask_id = match self.doc.mask_filter_id(host_id) {
            Some(id) => id,
            None => return,
        };

        // Save mask texture pixels to RegionScratch for undo before removing.
        let format = wgpu::TextureFormat::R8Unorm;
        let gpu_region_entry = if let Some((frame, rect)) = self
            .compositor
            .node_texture(mask_id)
            .map(|t| (t.canvas_frame(), t.canvas_extent()))
        {
            let snap = self.gpu.encode_ret("remove-mask-save", |encoder| {
                self.region_scratch
                    .save_region(&self.gpu.device, encoder, &frame, format, rect)
            });
            Some(commit_undo_region(
                &self.gpu,
                &self.region_scratch,
                &mut self.readbacks,
                "remove-mask-commit",
                mask_id,
                &frame,
                &snap,
                rect,
            ))
        } else {
            None
        };

        let detached = self.doc.detach_filter_for_undo(mask_id).is_some();
        // If the removed mask was the isolated/active node, clear the session flag.
        if self.isolated_node == Some(mask_id) {
            self.isolated_node = None;
        }
        self.compositor.dispose_node_texture(mask_id);
        self.compositor.dispose_passthrough_mask_state(host_id);
        self.compositor.dispose_projection_state(host_id);
        self.compositor.mark_dirty();

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::new();
        if let Some(entry) = gpu_region_entry {
            actions.push(Box::new(GpuRegionAction::new(entry)));
        }
        if detached {
            actions.push(Box::new(FilterRemoveAction::new(mask_id, host_id)));
        }
        if actions.len() == 1 {
            self.push_undo(actions.pop().unwrap());
        } else if !actions.is_empty() {
            self.push_undo(Box::new(CompoundAction::new(actions)));
        }
    }

    /// Bake the mask alpha into the host layer's RGBA, then remove the mask.
    /// Mask-specific — not generalized to "bake any filter" because that has
    /// no kind-uniform meaning.
    pub fn apply_mask(&mut self, host_id: LayerId) {
        if !self.doc.is_node_editable(host_id) {
            return;
        }
        // apply_mask is raster-only — groups have no pixel data to bake into.
        let host_is_raster = matches!(
            self.doc.find_node(host_id),
            Some(crate::layer::LayerNode::Layer(crate::layer::Layer::Raster(
                _
            )))
        );
        if !host_is_raster {
            return;
        }
        let mask_id = match self.doc.mask_filter_id(host_id) {
            Some(id) => id,
            None => return,
        };

        let format = wgpu::TextureFormat::Rgba8Unorm;

        // Save layer texture to region scratch for undo.
        let layer_frame = self
            .compositor
            .node_texture(host_id)
            .map(|t| t.canvas_frame());
        let snap = if let Some(frame) = layer_frame {
            let rect = frame.canvas_extent;
            Some(self.gpu.encode_ret("apply-mask-save", |encoder| {
                self.region_scratch
                    .save_region(&self.gpu.device, encoder, &frame, format, rect)
            }))
        } else {
            None
        };

        // Save the mask's R8 pixels too. The filter is removed at the end
        // of apply_mask; without this save, undo gets back the filter shell
        // with a fresh (all-white) mask texture and the user's painting on
        // the mask is lost forever. The actual GpuRegionAction is committed
        // and pushed AFTER `commit_region` for the host below — together
        // with the FilterRemoveAction — so undo replays them in the right
        // order: re-attach filter → restore mask pixels → restore host alpha.
        let mask_frame = self
            .compositor
            .node_texture(mask_id)
            .map(|t| t.canvas_frame());
        let mask_format = wgpu::TextureFormat::R8Unorm;
        let mask_snap = if let Some(frame) = mask_frame {
            let rect = frame.canvas_extent;
            Some(self.gpu.encode_ret("apply-mask-save-mask", |encoder| {
                self.region_scratch.save_region(
                    &self.gpu.device,
                    encoder,
                    &frame,
                    mask_format,
                    rect,
                )
            }))
        } else {
            None
        };

        // Create a bind group from the mask GPU texture for the multiply pass.
        let mask_bind_group = self.compositor.node_texture(mask_id).map(|mask_tex| {
            let sampler = self.gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mask-apply-sampler"),
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            self.paint_pipelines.create_selection_bind_group(
                &self.gpu.device,
                mask_tex.view(),
                &sampler,
            )
        });

        // GPU render pass: multiply layer alpha by mask values.
        if let (Some(layer_tex), Some(mask_bg)) = (
            self.compositor.node_texture(host_id),
            mask_bind_group.as_ref(),
        ) {
            let target = crate::gpu::paint_target::GpuPaintTarget::from_node(
                layer_tex,
                self.doc.canvas_rect(),
            );
            self.gpu.encode("apply-mask-multiply", |encoder| {
                target.multiply_alpha_by_mask(
                    encoder,
                    &self.paint_pipelines,
                    &self.gpu.queue,
                    mask_bg,
                );
            });
        }

        // Commit both undo regions (host alpha + mask pixels) before pushing
        // either, because the frames borrow `self.compositor` and `push_undo`
        // needs `&mut self` total. Push order is preserved: host first so
        // it pops last on undo. Each entry independently lands in the
        // `Pending → Ready` pipeline; the compound action becomes restorable
        // once each branch has either flipped to `Ready` or been hit by an
        // undo that consumes its staging buffer directly.
        let host_entry = if let (Some(snap), Some(frame)) = (snap, layer_frame) {
            let rect = frame.canvas_extent;
            Some(commit_undo_region(
                &self.gpu,
                &self.region_scratch,
                &mut self.readbacks,
                "apply-mask-undo",
                host_id,
                &frame,
                &snap,
                rect,
            ))
        } else {
            None
        };

        let mask_entry = if let (Some(snap), Some(frame)) = (mask_snap, mask_frame) {
            let rect = frame.canvas_extent;
            Some(commit_undo_region(
                &self.gpu,
                &self.region_scratch,
                &mut self.readbacks,
                "apply-mask-undo-mask",
                mask_id,
                &frame,
                &snap,
                rect,
            ))
        } else {
            None
        };

        if let Some(entry) = host_entry {
            self.push_undo(Box::new(GpuRegionAction::new(entry)));
        }
        if let Some(entry) = mask_entry {
            self.push_undo(Box::new(GpuRegionAction::new(entry)));
        }

        if self.isolated_node == Some(mask_id) {
            self.isolated_node = None;
        }

        // Apply baked the mask into the layer's alpha — layer pixels changed.
        self.compositor.mark_node_pixels_dirty(host_id);

        // Remove the filter from the document and its GPU texture. The
        // FilterRemoveAction is pushed last so undo pops it first — the
        // re-attach happens before sync_compositor_layers re-allocates the
        // R8 texture, after which the pending mask-region restore can land.
        let detached = self.doc.detach_filter_for_undo(mask_id).is_some();
        self.compositor.dispose_node_texture(mask_id);
        self.compositor.dispose_passthrough_mask_state(host_id);
        self.compositor.dispose_projection_state(host_id);
        if detached {
            self.push_undo(Box::new(FilterRemoveAction::new(mask_id, host_id)));
        }
    }

    /// Seed a host's mask from the active selection (creates the mask first
    /// if absent). Equivalent to `AddMask` followed by `copy_selection_into_mask`,
    /// but kept as a separate WASM API command for UX clarity.
    pub fn selection_to_mask(&mut self, host_id: LayerId) {
        if !self.doc.is_node_editable(host_id) {
            return;
        }
        // Add mask if not already present (idempotent on the second call).
        let already_had_mask = self.doc.has_mask(host_id);

        if !already_had_mask {
            // add_mask itself seeds from the active selection (see above), so
            // we're done after that single call.
            self.add_mask(host_id);
            return;
        }

        // Mask already exists — clone selection pixels into it.
        let mask_id = match self.doc.mask_filter_id(host_id) {
            Some(id) => id,
            None => return,
        };
        if let Some(src_id) = self.selection_modifier_id() {
            self.grow_filter(mask_id, self.selection_seed_bounds());
            self.clone_filter_pixels(src_id, mask_id);
        }
        self.compositor.mark_node_pixels_dirty(mask_id);
        self.compositor.mark_dirty();
    }

    /// Read a mask filter's pixels into the global selection. A straight
    /// GPU-to-GPU copy via [`Self::clone_filter_pixels`]; the CPU cache for
    /// the new selection contents is repopulated by the async
    /// `SelectionReadback` kicked at the end.
    pub fn mask_to_selection(&mut self, filter_id: LayerId) {
        if self.compositor.node_texture(filter_id).is_none() {
            return;
        }
        let dst = match self.selection_modifier_id() {
            Some(id) => id,
            None => return,
        };

        let was_active = self.has_selection();
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);

        self.clone_filter_pixels(filter_id, dst);
        self.set_selection_active(true);
        self.set_selection_pixel_bounds(None);
        self.invalidate_selection_cpu_cache();

        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }

    /// Resolve the filter id of the mask attached to a host, if any.
    /// Helper for callers (and tests) that hold a host id and want to operate
    /// on its mask without manually walking `doc.find_node(...).filters()`.
    pub fn host_mask_id(&self, host_id: LayerId) -> Option<LayerId> {
        self.doc.mask_filter_id(host_id)
    }

    /// Plane-space bounds to seed a mask from: the selection's tight pixel
    /// bounds (lifted from window-local), or the full canvas if no tight bounds
    /// are tracked (a whole-canvas selection).
    fn selection_seed_bounds(&self) -> crate::coord::CanvasRect {
        match self.selection_pixel_bounds() {
            Some(w) => w.to_canvas(self.doc.canvas_origin),
            None => self.doc.canvas_rect(),
        }
    }

    /// GPU-to-GPU copy of one filter's R8 pixel buffer into another's.
    /// Resolves source and destination via [`Self::modifier_frame`], so it
    /// works uniformly for any pair of pixel-bearing filter ids — selection
    /// or mask, in either direction. This is the §4a unification: the kind-
    /// specific bridge ops (`selection_to_mask`, `mask_to_selection`) now
    /// delegate to one function instead of duplicating the encode dance.
    ///
    /// **Clear-then-copy, bounds-aware.** The two textures may have different
    /// plane extents (a sub-canvas mask vs. the canvas-sized selection). `dst`
    /// is cleared to 0, then the plane-space *overlap* of the two extents is
    /// copied into `dst`'s local origin — so `dst` faithfully equals `src` over
    /// the overlap and reads 0 outside it, lossless across `dst`'s whole extent.
    /// Disjoint extents degrade to a pure clear, never a `copy range touches
    /// outside` validation crash. Callers that need the whole source represented
    /// grow `dst` to the union first (see `selection_to_mask` / `add_mask`).
    ///
    /// Marks `dst_id` thumbnail-dirty before returning per the write-site
    /// invariant — callers don't need to. For the selection filter (which
    /// doesn't surface in the layer panel), the mark is a no-op: the drain
    /// only readbacks ids present in `compositor.node_textures`.
    pub(crate) fn clone_filter_pixels(&mut self, src_id: LayerId, dst_id: LayerId) {
        let (src_frame, _) = match self.modifier_frame(src_id) {
            Some(v) => v,
            None => return,
        };
        let (dst_frame, dst_view) = match self.modifier_frame(dst_id) {
            Some(v) => v,
            None => return,
        };
        let overlap = src_frame.canvas_extent.intersect(dst_frame.canvas_extent);
        self.gpu.encode("clone-filter-pixels", |encoder| {
            // Clear the destination to 0 over its whole extent first.
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clone-filter-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            // Copy the plane overlap, mapped into each texture's local frame.
            if let Some(overlap) = overlap {
                if let (Some(src_local), Some(dst_local)) = (
                    src_frame.canvas_to_layer_rect(overlap),
                    dst_frame.canvas_to_layer_rect(overlap),
                ) {
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: src_frame.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: src_local.x0(),
                                y: src_local.y0(),
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: dst_frame.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: dst_local.x0(),
                                y: dst_local.y0(),
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: src_local.width.min(dst_local.width),
                            height: src_local.height.min(dst_local.height),
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
        });
        self.compositor.mark_node_pixels_dirty(dst_id);
    }

    /// Resolve a pixel-bearing filter id to a plane-space [`CanvasFrame`] plus
    /// its render view. The selection lives in `compositor.selection_state` and
    /// is window-local, so its extent is lifted to plane space via
    /// `canvas_origin`; per-host filters (mask, future filter/transform) live
    /// in the shared node-texture pool and are already plane-anchored.
    pub(crate) fn modifier_frame(
        &self,
        id: LayerId,
    ) -> Option<(crate::gpu::atlas::CanvasFrame<'_>, &wgpu::TextureView)> {
        if Some(id) == self.selection_modifier_id() {
            let s = self.compositor.selection_state()?;
            let extent = crate::coord::CanvasRect::from_xywh(
                self.doc.canvas_origin.x,
                self.doc.canvas_origin.y,
                s.width,
                s.height,
            );
            Some((
                crate::gpu::atlas::CanvasFrame {
                    texture: s.texture(),
                    canvas_extent: extent,
                },
                &s.views[s.current],
            ))
        } else {
            let t = self.compositor.node_texture(id)?;
            Some((t.canvas_frame(), t.view()))
        }
    }
}
