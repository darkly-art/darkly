//! Engine-level operations on mask filters.
//!
//! Add/remove/apply go through the generic `Document::add_mask_filter` /
//! `Document::detach_for_undo` helpers and the unified compositor node-texture
//! pool. Structural undo reuses the kind-uniform `EntityAddAction` /
//! `EntityRemoveAction`; only the mask's *pixels* need mask-specific handling,
//! via a `GpuRegionAction` in a `CompoundAction`. The "active node = paint
//! target" rule means there's no `editing_mask_layer` redirect: the active
//! node id directly identifies where strokes are routed.

use darkly_macros::handlers;

use super::super::rendering::commit_undo_region;
use super::super::DarklyEngine;
use crate::layer::LayerId;
use crate::undo::{
    CompoundAction, EntityAddAction, EntityRemoveAction, GpuRegionAction, MaskLinkedToHostAction,
    UndoAction,
};

#[handlers]
impl DarklyEngine {
    /// Attach a mask filter to a host layer or group, allocating its GPU
    /// texture in the unified node-texture pool. If a selection is active,
    /// the mask is seeded from the selection (one-click "selection → mask").
    #[handler]
    pub fn add_mask(&mut self, id: LayerId) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        // UI invariant: at most one mask per host. The model supports N; we
        // refuse here so that `add_mask_filter` doesn't silently create a
        // second one.
        // host unknown → bail (true keeps the existing semantics).
        if self.doc.find_node(id).is_none() {
            return;
        }
        if self.doc.has_mask(id) {
            return;
        }

        let Some(mod_id) = self.add_mask_unseeded(id) else {
            return;
        };

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

        let position = self.doc.position_in_parent(mod_id).unwrap_or(0);
        self.push_undo(Box::new(EntityAddAction::new(mod_id, Some(id), position)));
    }

    /// Allocate an empty (unseeded) mask filter on `id`: create the filter,
    /// allocate its R8 node texture, and ensure the per-host snapshot/lerp
    /// resource. Does NOT seed from the active selection and does NOT push an
    /// undo entry; callers frame it. [`Self::add_mask`] adds selection
    /// seeding and an `EntityAddAction`; duplicate and rich paste fold the mask
    /// into the subtree undo their own action already covers, so they must not
    /// seed from (nor consume) the receiving document's selection.
    ///
    /// Returns the new mask filter id, or `None` if the host can't take one.
    pub(crate) fn add_mask_unseeded(&mut self, id: LayerId) -> Option<LayerId> {
        let mod_id = self.doc.add_mask_filter(id)?;
        let bounds = self
            .doc
            .find_filter(mod_id)
            .and_then(|m| m.pixels())?
            .bounds;
        self.compositor.ensure_node_texture(
            &self.gpu.device,
            &self.gpu.queue,
            mod_id,
            wgpu::TextureFormat::R8Unorm,
            bounds,
        );
        // Per-host snapshot+lerp resource for the in-place masked-host path
        // (passthrough group or filter layer). Idempotent across every host
        // kind; only the in-place composite paths consume it, but the engine
        // doesn't need to branch: the compositor reads it lazily.
        self.compositor
            .ensure_mask_snapshot_state(&self.gpu.device, id);
        Some(mod_id)
    }

    /// Set the transform relationship state owned by a mask filter entity.
    #[handler]
    pub fn set_mask_linked_to_host(&mut self, id: LayerId, linked: bool) {
        if !self.resolve_transform_conflict() || !self.doc.is_node_editable(id) {
            return;
        }
        let old = match self.doc.find_filter_mut(id) {
            Some(filter) => match &mut filter.kind {
                crate::document::FilterKind::Mask(mask) if mask.linked_to_host != linked => {
                    let old = mask.linked_to_host;
                    mask.linked_to_host = linked;
                    old
                }
                _ => return,
            },
            None => return,
        };
        self.push_undo(Box::new(MaskLinkedToHostAction::new(id, old)));
    }

    /// Remove the mask filter on a host layer or group.
    #[handler]
    pub fn remove_mask(&mut self, id: LayerId) {
        if !self.resolve_transform_conflict() || !self.doc.is_node_editable(id) {
            return;
        }
        let Some(mask_id) = self.doc.mask_filter_id(id) else {
            return;
        };
        self.remove_modifier(mask_id);
    }

    /// Remove a modifier addressed by its **own** id rather than its host's.
    /// The layer panel lists modifiers as selectable rows, so generic
    /// operations (delete, batch delete) reach them this way.
    pub(crate) fn remove_modifier(&mut self, modifier_id: LayerId) {
        if !self.resolve_transform_conflict() {
            return;
        }
        if let Some(action) = self.detach_for_remove(modifier_id) {
            self.push_undo(action);
        }
    }

    /// Detach a modifier and return the matching undo action without pushing
    /// it: the modifier-kind counterpart of `detach_for_remove`, so batch
    /// removal can fold modifiers into one undo step alongside layers.
    ///
    /// Owns the bookkeeping a modifier's pixels need and a tree node's don't:
    /// the mask texture is saved into a region entry (so undo can restore the
    /// bytes) and disposed eagerly, which is why the structural action carries
    /// no tombstones.
    pub(crate) fn detach_modifier_for_remove(
        &mut self,
        modifier_id: LayerId,
    ) -> Option<Box<dyn UndoAction>> {
        let host_id = self.doc.parent_of(modifier_id)?;
        if !self.doc.is_node_editable(host_id) {
            return None;
        }

        // Save mask texture pixels to RegionScratch for undo before removing.
        let format = wgpu::TextureFormat::R8Unorm;
        let gpu_region_entry = if let Some((frame, rect)) = self
            .compositor
            .node_texture(modifier_id)
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
                modifier_id,
                &frame,
                &snap,
                rect,
            ))
        } else {
            None
        };

        // Captured before the detach severs the parent link the position is
        // read from, so undo restores the mask at its original index.
        let mask_position = self.doc.position_in_parent(modifier_id).unwrap_or(0);
        let detached = self.doc.detach_for_undo(modifier_id).is_some();
        self.compositor.dispose_node_texture(modifier_id);
        self.compositor.dispose_mask_snapshot_state(host_id);
        self.compositor.dispose_projection_state(host_id);
        self.compositor.mark_dirty();

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::new();
        if let Some(entry) = gpu_region_entry {
            actions.push(Box::new(GpuRegionAction::new(entry)));
        }
        if detached {
            actions.push(Box::new(EntityRemoveAction::new(
                modifier_id,
                Some(host_id),
                mask_position,
                Vec::new(),
            )));
        }
        match actions.len() {
            0 => None,
            1 => actions.pop(),
            _ => Some(Box::new(CompoundAction::new(actions))),
        }
    }

    /// Bake the mask alpha into the host layer's RGBA, then remove the mask.
    /// Mask-specific, not generalized to "bake any filter" because that has
    /// no kind-uniform meaning.
    #[handler]
    pub fn apply_mask(&mut self, id: LayerId) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        // apply_mask is raster-only: groups have no pixel data to bake into.
        let host_is_raster = matches!(
            self.doc.find_node(id),
            Some(crate::layer::LayerNode::Layer(crate::layer::Layer::Raster(
                _
            )))
        );
        if !host_is_raster {
            return;
        }
        let mask_id = match self.doc.mask_filter_id(id) {
            Some(id) => id,
            None => return,
        };

        let format = wgpu::TextureFormat::Rgba8Unorm;

        // Save layer texture to region scratch for undo.
        let layer_frame = self.compositor.node_texture(id).map(|t| t.canvas_frame());
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
        // the mask is lost forever. Its GpuRegionAction is bundled below into
        // the single CompoundAction alongside the host-alpha region and the
        // EntityRemoveAction, so one undo replays them in the right order:
        // re-attach filter → restore mask pixels → restore host alpha.
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

        // GPU render pass: multiply layer alpha by mask values, sampling the
        // mask in its OWN plane-anchored frame (`mask_rect`) so the bake matches
        // the live composite. A mask *filter* texture is not canvas-window-sized
        // (its extent is the host's bounds, possibly grown), so the generic
        // `multiply_alpha_by_mask` selection-frame sampling would land the mask
        // at shifted/scaled texels whenever the frames diverge (sub-canvas
        // layer, grown mask, cropped canvas).
        if let (Some(layer_tex), Some(mask_bg), Some(mask_rect)) = (
            self.compositor.node_texture(id),
            mask_bind_group.as_ref(),
            mask_frame.as_ref().map(|f| f.canvas_extent),
        ) {
            let target = crate::gpu::paint_target::GpuPaintTarget::from_node(
                layer_tex,
                self.doc.canvas_rect(),
            );
            let mut encoder = crate::gpu::paint_target::PaintCommandEncoder::new(
                &self.gpu.device,
                &self.gpu.queue,
                &self.paint_pipelines,
                "apply-mask-multiply",
                1,
            );
            target.multiply_alpha_by_mask_in_frame(
                &mut encoder,
                &self.paint_pipelines,
                &self.gpu.queue,
                mask_bg,
                mask_rect,
            );
            encoder.submit();
        }

        // Commit both undo regions (host alpha + mask pixels) before building
        // the compound, because the frames borrow `self.compositor` and
        // `push_undo` needs `&mut self` total. They are collected in the order
        // `[host, mask, filterRemove]` so `CompoundAction::undo` (reverse)
        // replays them as filterRemove → mask → host. Each region entry
        // independently lands in the `Pending → Ready` pipeline; the compound
        // becomes restorable once each branch has either flipped to `Ready` or
        // been hit by an undo that consumes its staging buffer directly.
        let host_entry = if let (Some(snap), Some(frame)) = (snap, layer_frame) {
            let rect = frame.canvas_extent;
            Some(commit_undo_region(
                &self.gpu,
                &self.region_scratch,
                &mut self.readbacks,
                "apply-mask-undo",
                id,
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

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::new();
        if let Some(entry) = host_entry {
            actions.push(Box::new(GpuRegionAction::new(entry)));
        }
        if let Some(entry) = mask_entry {
            actions.push(Box::new(GpuRegionAction::new(entry)));
        }

        if self.isolated_node == Some(mask_id) {
            self.isolated_node = None;
        }

        // Apply baked the mask into the layer's alpha: layer pixels changed.
        self.compositor.mark_node_pixels_dirty(id);

        // Remove the filter from the document and its GPU texture, then bundle
        // the EntityRemoveAction last in the vec so `CompoundAction::undo`
        // (reverse) pops it first: the re-attach happens before
        // sync_compositor_layers re-allocates the R8 texture, after which the
        // pending mask-region restore can land.
        // Captured before the detach severs the parent link the position is
        // read from, so undo restores the mask at its original index.
        let mask_position = self.doc.position_in_parent(mask_id).unwrap_or(0);
        let detached = self.doc.detach_for_undo(mask_id).is_some();
        self.compositor.dispose_node_texture(mask_id);
        self.compositor.dispose_mask_snapshot_state(id);
        self.compositor.dispose_projection_state(id);
        if detached {
            actions.push(Box::new(EntityRemoveAction::new(
                mask_id,
                Some(id),
                mask_position,
                Vec::new(),
            )));
        }

        // One Apply Mask = one undo step: fold the host-alpha region, mask-pixel
        // region, and filter-detach into a single CompoundAction (or push the
        // lone action directly when only one survived).
        if actions.len() == 1 {
            self.push_undo(actions.pop().unwrap());
        } else if !actions.is_empty() {
            self.push_undo(Box::new(CompoundAction::new(actions)));
        }
    }

    /// Seed a host's mask from the active selection (creates the mask first
    /// if absent). Equivalent to `AddMask` followed by `copy_selection_into_mask`,
    /// but kept as a separate WASM API command for UX clarity.
    #[handler]
    pub fn selection_to_mask(&mut self, id: LayerId) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        // Add mask if not already present (idempotent on the second call).
        let already_had_mask = self.doc.has_mask(id);

        if !already_had_mask {
            // add_mask itself seeds from the active selection (see above), so
            // we're done after that single call.
            self.add_mask(id);
            return;
        }

        // Mask already exists: clone selection pixels into it.
        let mask_id = match self.doc.mask_filter_id(id) {
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
    #[handler]
    pub fn mask_to_selection(&mut self, id: LayerId) {
        if self.compositor.node_texture(id).is_none() {
            return;
        }
        let dst = match self.selection_modifier_id() {
            Some(id) => id,
            None => return,
        };

        let was_active = self.has_selection();
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);

        self.clone_filter_pixels(id, dst);
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
    /// works uniformly for any pair of pixel-bearing filter ids, selection
    /// or mask, in either direction. This is the §4a unification: the kind-
    /// specific bridge ops (`selection_to_mask`, `mask_to_selection`) now
    /// delegate to one function instead of duplicating the encode dance.
    ///
    /// **Clear-then-copy, bounds-aware.** The two textures may have different
    /// plane extents (a sub-canvas mask vs. the canvas-sized selection). `dst`
    /// is cleared to 0, then the plane-space *overlap* of the two extents is
    /// copied into `dst`'s local origin, so `dst` faithfully equals `src` over
    /// the overlap and reads 0 outside it, lossless across `dst`'s whole extent.
    /// Disjoint extents degrade to a pure clear, never a `copy range touches
    /// outside` validation crash. Callers that need the whole source represented
    /// grow `dst` to the union first (see `selection_to_mask` / `add_mask`).
    ///
    /// Marks `dst_id` thumbnail-dirty before returning per the write-site
    /// invariant; callers don't need to. For the selection filter (which
    /// doesn't surface in the layer panel), the mark is a no-op: the drain
    /// only readbacks ids present in `compositor.node_textures`.
    pub(crate) fn clone_filter_pixels(&mut self, src_id: LayerId, dst_id: LayerId) {
        let linked_to_host = self.doc.find_filter(src_id).and_then(|filter| {
            if let crate::document::FilterKind::Mask(mask) = &filter.kind {
                Some(mask.linked_to_host)
            } else {
                None
            }
        });
        if let (Some(linked), Some(filter)) = (linked_to_host, self.doc.find_filter_mut(dst_id)) {
            if let crate::document::FilterKind::Mask(mask) = &mut filter.kind {
                mask.linked_to_host = linked;
            }
        }

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
