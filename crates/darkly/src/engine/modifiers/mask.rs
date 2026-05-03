//! Engine-level operations on mask modifiers.
//!
//! Replaces the old `engine/masks.rs`. Add/remove/apply now go through the
//! generic `Document::add_mask_modifier` / `Document::remove_modifier` helpers
//! and the unified compositor node-texture pool. The "active node = paint
//! target" rule means there's no `editing_mask_layer` redirect — the active
//! node id directly identifies where strokes are routed.

use super::super::{DarklyEngine, ReadbackContext};
use crate::gpu::readback;
use crate::layer::LayerId;
use crate::undo::{
    CompoundAction, GpuRegionAction, ModifierAddAction, ModifierRemoveAction, UndoAction,
};

impl DarklyEngine {
    /// Attach a mask modifier to a host layer or group, allocating its GPU
    /// texture in the unified node-texture pool. If a selection is active,
    /// the mask is seeded from the selection (one-click "selection → mask").
    pub fn add_mask(&mut self, host_id: LayerId) {
        // UI invariant: at most one mask per host. The model supports N; we
        // refuse here so that `add_mask_modifier` doesn't silently create a
        // second one.
        if self
            .doc
            .find_node(host_id)
            .map(|n| n.modifiers().mask().is_some())
            .unwrap_or(true)
        {
            return;
        }

        let mod_id = match self.doc.add_mask_modifier(host_id) {
            Some(id) => id,
            None => return,
        };

        let bounds = match self.doc.find_modifier(mod_id).and_then(|m| m.pixels()) {
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

        // If a selection is active, seed the mask pixels from the selection
        // texture (R8 → R8 full-canvas copy).
        if self.gpu_selection.active {
            self.copy_selection_into_mask(mod_id);
        }

        // Fresh mask texture (and possibly a selection-seeded copy on top) —
        // queue a thumbnail readback so the panel's mask thumbnail appears
        // immediately rather than waiting for the next paint stroke.
        self.compositor.mark_node_pixels_dirty(mod_id);
        self.compositor.mark_dirty();

        self.undo_stack
            .push(Box::new(ModifierAddAction::new(mod_id, host_id)));
    }

    /// GPU-to-GPU copy of the active selection texture into a mask modifier's
    /// texture (R8 → R8, full canvas). No-op if no selection is active or
    /// the modifier has no GPU texture.
    fn copy_selection_into_mask(&mut self, modifier_id: LayerId) {
        if !self.gpu_selection.active {
            return;
        }
        let Some(mask_tex) = self.compositor.node_texture(modifier_id) else {
            return;
        };
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        self.gpu.encode("sel-to-mask-copy", |encoder| {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self.gpu_selection.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &mask_tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: canvas_w,
                    height: canvas_h,
                    depth_or_array_layers: 1,
                },
            );
        });
    }

    /// Remove the mask modifier on a host layer or group.
    pub fn remove_mask(&mut self, host_id: LayerId) {
        let mask_id = match self
            .doc
            .find_node(host_id)
            .and_then(|n| n.modifiers().mask().map(|m| m.id))
        {
            Some(id) => id,
            None => return,
        };

        // Save mask texture pixels to RegionStore for undo before removing.
        let format = wgpu::TextureFormat::R8Unorm;
        let gpu_region_entry = if let Some(mask_tex) = self.compositor.node_texture(mask_id) {
            let frame = mask_tex.canvas_frame();
            let rect = frame.canvas_extent;
            let mut entry = None;
            self.gpu.encode("remove-mask-save", |encoder| {
                let snap = self.region_store.save_region(encoder, &frame, format, rect);
                entry = Some(
                    self.region_store
                        .commit_region(encoder, mask_id, &frame, &snap, rect),
                );
            });
            entry
        } else {
            None
        };

        let detached = self.doc.remove_modifier(mask_id);
        // If the removed mask was the isolated/active node, clear the session flag.
        if self.isolated_node == Some(mask_id) {
            self.isolated_node = None;
        }
        self.compositor.dispose_node_texture(mask_id);
        self.compositor.dispose_passthrough_mask_state(host_id);
        self.compositor.mark_dirty();

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::new();
        if let Some(entry) = gpu_region_entry {
            actions.push(Box::new(GpuRegionAction::new(entry)));
        }
        if let Some(modifier) = detached {
            actions.push(Box::new(ModifierRemoveAction::new(modifier, host_id)));
        }
        if actions.len() == 1 {
            self.undo_stack.push(actions.pop().unwrap());
        } else if !actions.is_empty() {
            self.undo_stack.push(Box::new(CompoundAction::new(actions)));
        }
    }

    /// Bake the mask alpha into the host layer's RGBA, then remove the mask.
    /// Mask-specific — not generalized to "bake any modifier" because that has
    /// no kind-uniform meaning.
    pub fn apply_mask(&mut self, host_id: LayerId) {
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
        let mask_id = match self
            .doc
            .find_node(host_id)
            .and_then(|n| n.modifiers().mask().map(|m| m.id))
        {
            Some(id) => id,
            None => return,
        };

        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let format = wgpu::TextureFormat::Rgba8Unorm;

        // Save layer texture to region store for undo.
        let layer_frame = self
            .compositor
            .node_texture(host_id)
            .map(|t| t.canvas_frame());
        let snap = if let Some(frame) = layer_frame {
            let rect = frame.canvas_extent;
            Some(self.gpu.encode_ret("apply-mask-save", |encoder| {
                self.region_store.save_region(encoder, &frame, format, rect)
            }))
        } else {
            None
        };

        // Save the mask's R8 pixels too. The modifier is removed at the end
        // of apply_mask; without this save, undo gets back the modifier shell
        // with a fresh (all-white) mask texture and the user's painting on
        // the mask is lost forever. The actual GpuRegionAction is committed
        // and pushed AFTER `commit_region` for the host below — together
        // with the ModifierRemoveAction — so undo replays them in the right
        // order: re-attach modifier → restore mask pixels → restore host alpha.
        let mask_frame = self
            .compositor
            .node_texture(mask_id)
            .map(|t| t.canvas_frame());
        let mask_format = wgpu::TextureFormat::R8Unorm;
        let mask_snap = if let Some(frame) = mask_frame {
            let rect = frame.canvas_extent;
            Some(self.gpu.encode_ret("apply-mask-save-mask", |encoder| {
                self.region_store
                    .save_region(encoder, &frame, mask_format, rect)
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
                &mask_tex.view,
                &sampler,
            )
        });

        // GPU render pass: multiply layer alpha by mask values.
        if let (Some(layer_tex), Some(mask_bg)) = (
            self.compositor.node_texture(host_id),
            mask_bind_group.as_ref(),
        ) {
            let target =
                crate::gpu::paint_target::GpuPaintTarget::from_node(layer_tex, canvas_w, canvas_h);
            self.gpu.encode("apply-mask-multiply", |encoder| {
                target.multiply_alpha_by_mask(
                    encoder,
                    &self.paint_pipelines,
                    &self.gpu.queue,
                    mask_bg,
                );
            });
        }

        // Commit undo region for the host's pixel changes (pushed first so
        // it pops last — restores host alpha after the modifier and its
        // pixels have been brought back).
        if let (Some(snap), Some(frame)) = (snap, layer_frame) {
            let rect = frame.canvas_extent;
            self.gpu.encode("apply-mask-undo", |encoder| {
                let entry = self
                    .region_store
                    .commit_region(encoder, host_id, &frame, &snap, rect);
                self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
            });
        }

        // Commit undo region for the mask pixels (pushed second so it pops
        // after the modifier is reattached, into the freshly-recreated
        // mask texture).
        if let (Some(snap), Some(frame)) = (mask_snap, mask_frame) {
            let rect = frame.canvas_extent;
            self.gpu.encode("apply-mask-undo-mask", |encoder| {
                let entry = self
                    .region_store
                    .commit_region(encoder, mask_id, &frame, &snap, rect);
                self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
            });
        }

        if self.isolated_node == Some(mask_id) {
            self.isolated_node = None;
        }

        // Apply baked the mask into the layer's alpha — layer pixels changed.
        self.compositor.mark_node_pixels_dirty(host_id);

        // Remove the modifier from the document and its GPU texture. The
        // ModifierRemoveAction is pushed last so undo pops it first — the
        // re-attach happens before sync_compositor_layers re-allocates the
        // R8 texture, after which the pending mask-region restore can land.
        let detached = self.doc.remove_modifier(mask_id);
        self.compositor.dispose_node_texture(mask_id);
        self.compositor.dispose_passthrough_mask_state(host_id);
        if let Some(modifier) = detached {
            self.undo_stack
                .push(Box::new(ModifierRemoveAction::new(modifier, host_id)));
        }
    }

    /// Seed a host's mask from the active selection (creates the mask first
    /// if absent). Equivalent to `AddMask` followed by `copy_selection_into_mask`,
    /// but kept as a separate WASM API command for UX clarity.
    pub fn selection_to_mask(&mut self, host_id: LayerId) {
        // Add mask if not already present (idempotent on the second call).
        let already_had_mask = self
            .doc
            .find_node(host_id)
            .map(|n| n.modifiers().mask().is_some())
            .unwrap_or(false);

        if !already_had_mask {
            // add_mask itself seeds from the active selection (see above), so
            // we're done after that single call.
            self.add_mask(host_id);
            return;
        }

        // Mask already exists — copy selection into it.
        let mask_id = match self
            .doc
            .find_node(host_id)
            .and_then(|n| n.modifiers().mask().map(|m| m.id))
        {
            Some(id) => id,
            None => return,
        };
        self.copy_selection_into_mask(mask_id);
        self.compositor.mark_node_pixels_dirty(mask_id);
        self.compositor.mark_dirty();
    }

    /// Read a mask modifier's pixels back into the active selection.
    pub fn mask_to_selection(&mut self, modifier_id: LayerId) {
        if self.compositor.node_texture(modifier_id).is_none() {
            return;
        }

        let was_active = self.gpu_selection.active;
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);

        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        let mask_tex = self.compositor.node_texture(modifier_id).unwrap();
        self.gpu.encode("mask-to-sel-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                &mask_tex.texture,
                wgpu::TextureFormat::R8Unorm,
                [0, 0, canvas_w, canvas_h],
            );
            self.readbacks
                .submit(request, ReadbackContext::MaskToSelection { was_active });
        });
    }

    /// Resolve the modifier id of the mask attached to a host, if any.
    /// Helper for callers (and tests) that hold a host id and want to operate
    /// on its mask without manually walking `doc.find_node(...).modifiers()`.
    pub fn host_mask_id(&self, host_id: LayerId) -> Option<LayerId> {
        self.doc
            .find_node(host_id)
            .and_then(|n| n.modifiers().mask().map(|m| m.id))
    }

    /// Complete mask-to-selection after async readback.
    pub(crate) fn complete_mask_to_selection(&mut self, was_active: bool, pixels: Vec<u8>) {
        self.gpu_selection.upload_replace_full(
            &self.gpu.device,
            &self.gpu.queue,
            &pixels,
            self.brush_pipelines.selection_bind_group_layout(),
            &self.paint_pipelines.selection_bind_group_layout,
        );

        let rect = self.selection_full_canvas_rect();
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }
}
