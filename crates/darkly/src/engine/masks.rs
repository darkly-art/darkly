//! Layer mask operations — add, remove, apply, toggle, mask↔selection.

use super::{DarklyEngine, ReadbackContext};
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback;
use crate::layer::{Layer, LayerNode};
use crate::undo::{CompoundAction, GpuRegionAction, MaskPropertyAction};

impl DarklyEngine {
    pub fn add_mask(&mut self, layer_id: u64) {
        let snap = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().mask_snapshot(),
            None => return,
        };

        self.doc.add_mask(layer_id);
        self.compositor
            .set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id,
            snap.has_mask,
            snap.mask_enabled,
            snap.show_mask,
        )));
    }

    pub fn remove_mask(&mut self, layer_id: u64) {
        let snap = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().mask_snapshot(),
            None => return,
        };
        if !snap.has_mask {
            return;
        }

        // Save mask texture pixels to RegionStore for undo before removing.
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let format = wgpu::TextureFormat::R8Unorm;
        let rect = crate::coord::LayerRect::from_xywh(0, 0, canvas_w, canvas_h);
        let gpu_region_entry = if let Some(mask_tex) = self.compositor.mask_texture(layer_id) {
            let mut entry = None;
            self.gpu.encode("remove-mask-save", |encoder| {
                let snap = self
                    .region_store
                    .save_region(encoder, &mask_tex.texture, format, rect);
                entry = Some(
                    self.region_store
                        .commit_region(encoder, layer_id, &snap, rect),
                );
            });
            entry
        } else {
            None
        };

        self.doc.remove_mask(layer_id);
        self.editing_mask_layer = self.editing_mask_layer.filter(|&id| id != layer_id);
        self.compositor
            .set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, false);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        // Wrap GpuRegionAction + MaskPropertyAction in a CompoundAction so
        // undo restores both the mask flag and the mask pixel data.
        let mask_action = Box::new(MaskPropertyAction::new(
            layer_id,
            snap.has_mask,
            snap.mask_enabled,
            snap.show_mask,
        ));
        if let Some(entry) = gpu_region_entry {
            let region_action = Box::new(GpuRegionAction::new(entry));
            self.undo_stack.push(Box::new(CompoundAction::new(vec![
                region_action,
                mask_action,
            ])));
        } else {
            self.undo_stack.push(mask_action);
        }
    }

    pub fn apply_mask(&mut self, layer_id: u64) {
        // apply_mask is raster-only — groups have no pixel data to bake into
        let (old_has, old_enabled, old_show) = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => (r.has_mask, r.mask_enabled, r.show_mask),
            _ => return,
        };
        if !old_has {
            return;
        }

        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let rect = crate::coord::LayerRect::from_xywh(0, 0, canvas_w, canvas_h);

        // Save layer texture to region store for undo.
        let snap = if let Some(layer_tex) = self.compositor.layer_texture(layer_id) {
            Some(self.gpu.encode_ret("apply-mask-save", |encoder| {
                self.region_store
                    .save_region(encoder, &layer_tex.texture, format, rect)
            }))
        } else {
            None
        };

        // Create a bind group from the mask GPU texture for the multiply pass.
        let mask_bind_group = self.compositor.mask_texture(layer_id).map(|mask_tex| {
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
            self.compositor.layer_texture(layer_id),
            mask_bind_group.as_ref(),
        ) {
            let target = GpuPaintTarget::from_layer(layer_tex, canvas_w, canvas_h);
            self.gpu.encode("apply-mask-multiply", |encoder| {
                target.multiply_alpha_by_mask(
                    encoder,
                    &self.paint_pipelines,
                    &self.gpu.queue,
                    mask_bg,
                );
            });
        }

        // Commit undo region.
        if let Some(snap) = snap {
            self.gpu.encode("apply-mask-undo", |encoder| {
                let entry = self
                    .region_store
                    .commit_region(encoder, layer_id, &snap, rect);
                self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
            });
        }

        self.editing_mask_layer = self.editing_mask_layer.filter(|&id| id != layer_id);
        self.compositor
            .set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, false);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        // Also remove mask on document side
        self.doc.remove_mask(layer_id);

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id,
            old_has,
            old_enabled,
            old_show,
        )));
    }

    pub fn set_mask_enabled(&mut self, layer_id: u64, enabled: bool) {
        let snap = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().mask_snapshot(),
            None => return,
        };
        self.doc.set_mask_enabled(layer_id, enabled);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id,
            snap.has_mask,
            snap.mask_enabled,
            snap.show_mask,
        )));
    }

    pub fn set_show_mask(&mut self, layer_id: u64, show: bool) {
        let snap = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().mask_snapshot(),
            None => return,
        };
        self.doc.set_show_mask(layer_id, show);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id,
            snap.has_mask,
            snap.mask_enabled,
            snap.show_mask,
        )));
    }

    pub fn set_editing_mask(&mut self, layer_id: u64, editing: bool) {
        if editing {
            self.editing_mask_layer = Some(layer_id);
        } else if self.editing_mask_layer == Some(layer_id) {
            self.editing_mask_layer = None;
        }
    }

    pub fn selection_to_mask(&mut self, layer_id: u64) {
        let snap = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().mask_snapshot(),
            None => return,
        };

        // Set mask flags directly (doc.selection_to_mask guards on doc.selection
        // which we no longer maintain — the guard is now gpu_selection.active,
        // checked by the caller).
        self.doc.add_mask(layer_id);
        self.compositor
            .set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);

        // Copy selection texture → mask texture on GPU. Both are R8, same
        // canvas dimensions. No CPU round-trip needed.
        if self.gpu_selection.active {
            if let Some(mask_tex) = self.compositor.mask_texture(layer_id) {
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
        }

        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id,
            snap.has_mask,
            snap.mask_enabled,
            snap.show_mask,
        )));
    }

    pub fn mask_to_selection(&mut self, layer_id: u64) {
        if self.compositor.mask_texture(layer_id).is_none() {
            return;
        }

        let was_active = self.gpu_selection.active;
        // Mask-to-selection uploads a full-canvas buffer — reserve a full-canvas
        // undo rect so the matching commit in complete_mask_to_selection aligns.
        let rect = self.selection_full_canvas_rect();
        self.save_selection_for_undo(rect);

        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        let mask_tex = self.compositor.mask_texture(layer_id).unwrap();
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

    /// Complete mask-to-selection after async readback.
    pub(crate) fn complete_mask_to_selection(&mut self, was_active: bool, pixels: Vec<u8>) {
        // Upload the mask data directly to the GPU selection texture.
        self.gpu_selection.upload_replace_full(
            &self.gpu.device,
            &self.gpu.queue,
            &pixels,
            self.brush_pipelines.selection_bind_group_layout(),
            &self.paint_pipelines.selection_bind_group_layout,
        );

        // Matches the full-canvas save reserved in `mask_to_selection`.
        let rect = self.selection_full_canvas_rect();
        self.commit_selection_undo(was_active, rect);
        self.kick_selection_readback();
    }

    /// Sync compositor mask state (bind group + uniforms) for a layer or group.
    pub(crate) fn sync_mask_state(&mut self, layer_id: u64) {
        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let has_mask = m.has_mask();
        let mask_enabled = m.mask_enabled();
        let show_mask = m.show_mask();

        self.compositor
            .set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, has_mask);
        self.compositor
            .update_mask_binding(&self.gpu.device, layer_id, mask_enabled, show_mask);

        // Update uniforms for the appropriate cache type
        match node {
            LayerNode::Layer(Layer::Raster(r)) => {
                self.compositor.update_raster_uniforms_full(
                    &self.gpu.queue,
                    layer_id,
                    r.opacity,
                    r.blend_mode,
                    show_mask,
                );
            }
            LayerNode::Group(g) => {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue,
                    layer_id,
                    g.opacity,
                    g.blend_mode,
                    show_mask,
                );
            }
        }
    }
}
