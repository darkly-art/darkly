//! Floating content — paste-in-place and interactive transforms.

use super::{DarklyEngine, PendingTransform};
use crate::document::MoveTarget;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::transform::{Affine2D, FloatingContent, FloatingMode, IDENTITY};
use crate::layer::Layer;
use crate::undo::{GpuRegionAction, LayerAddAction};

impl DarklyEngine {
    /// Auto-commit any active floating content before performing other edits.
    /// Call this before operations that would conflict with floating content
    /// (layer switch, paint, undo, etc.).
    pub fn auto_commit_floating(&mut self) {
        if self.floating.is_some() {
            self.commit_floating();
        }
    }

    /// Check if there is active floating content.
    pub fn has_floating(&self) -> bool {
        self.floating.is_some()
    }

    /// Return floating content info for the frontend overlay:
    /// (source_origin_x, source_origin_y, source_width, source_height, matrix[6]).
    /// Returns None if no floating content is active.
    pub fn floating_info(&self) -> Option<(f32, f32, f32, f32, Affine2D)> {
        self.floating.as_ref().map(|fc| {
            (
                fc.source_origin.0 as f32,
                fc.source_origin.1 as f32,
                fc.source_width as f32,
                fc.source_height as f32,
                fc.matrix,
            )
        })
    }

    /// Return the layer the active floating content will commit to.
    /// Used by the frontend to distinguish "user switched away from the
    /// floating's layer" (dismiss) from "user activated the floating's
    /// own target layer" (keep — paste-as-floating sets active to its
    /// auto-created target).
    pub fn floating_target_layer(&self) -> Option<u64> {
        self.floating.as_ref().map(|fc| fc.target_layer)
    }

    /// Paste from the internal clipboard as floating content on the current
    /// layer/mask. Returns true if floating content was created.
    pub fn paste_in_place_floating(&mut self, layer_id: u64) -> bool {
        // Auto-commit any existing floating content first.
        self.auto_commit_floating();

        let clip = match self.clipboard.as_ref().and_then(|c| c.as_image()) {
            Some(c) => c,
            None => return false,
        };

        let target_is_mask = self.editing_mask_layer == Some(layer_id);

        let source_origin = (clip.offset_x, clip.offset_y);
        let source_width = clip.width;
        let source_height = clip.height;

        // Upload flat RGBA data to GPU for preview.
        self.compositor.set_floating_content(
            &self.gpu.device,
            &self.gpu.queue,
            &clip.data,
            source_origin,
            source_width,
            source_height,
            layer_id,
            target_is_mask,
        );

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            target_is_mask,
            mode: FloatingMode::Paste {
                created_layer_id: None,
            },
        });

        true
    }

    /// Paste raw RGBA bytes as floating content on a NEW raster layer.
    /// The caller is expected to switch to the transform tool. On commit, the
    /// pixel data is rendered into the new layer and a single LayerAddAction
    /// is pushed to undo. On cancel, the new layer is removed silently.
    ///
    /// Returns the new layer id.
    pub fn paste_image_floating(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: Option<u64>,
    ) -> u64 {
        // Auto-commit any existing floating content first.
        self.auto_commit_floating();

        // Size the new layer to fit the paste, so off-canvas pixels are
        // preserved when the floating commits.
        let layer_bounds = crate::layer::LayerBounds {
            offset_x,
            offset_y,
            width,
            height,
        };

        // Create the target layer (no undo entry yet — pushed at commit).
        let new_id = self.doc.add_raster_layer();
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(new_id) {
            r.name = "Pasted Layer".to_string();
            r.bounds = layer_bounds;
        }
        self.compositor.ensure_raster_layer(
            &self.gpu.device,
            &self.gpu.queue,
            new_id,
            layer_bounds,
        );

        if let Some(active_id) = active_layer_id {
            self.doc.move_layer(new_id, MoveTarget::After(active_id));
        }

        // Upload RGBA to floating source texture; the compositor renders it
        // as a preview overlay until commit.
        self.compositor.set_floating_content(
            &self.gpu.device,
            &self.gpu.queue,
            rgba,
            (offset_x, offset_y),
            width,
            height,
            new_id,
            false,
        );

        self.floating = Some(FloatingContent {
            source_origin: (offset_x, offset_y),
            source_width: width,
            source_height: height,
            matrix: IDENTITY,
            target_layer: new_id,
            target_is_mask: false,
            mode: FloatingMode::Paste {
                created_layer_id: Some(new_id),
            },
        });

        new_id
    }

    /// Begin an interactive transform on the current layer/mask content.
    ///
    /// When a selection is active, source bounds come from the selection and
    /// the transform is set up synchronously (returns true).
    ///
    /// When there is no selection, content bounds are needed from the
    /// compositor's GPU compute system. If cached, setup is synchronous.
    /// Otherwise, an async compute is dispatched and the transform completes
    /// on the next frame via `poll_pending`.
    pub fn begin_transform(&mut self, layer_id: u64) -> bool {
        self.auto_commit_floating();

        let target_is_mask = self.editing_mask_layer == Some(layer_id);

        if self.doc.layer(layer_id).is_none() {
            return false;
        }
        if target_is_mask {
            let has_mask = matches!(self.doc.layer(layer_id), Some(Layer::Raster(r)) if r.has_mask);
            if !has_mask {
                return false;
            }
        }

        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Determine source bounds.
        if self.gpu_selection.active {
            // Selection bounds come from cpu_cache (populated eagerly on
            // upload or lazily from async readback).  If unavailable, defer.
            if self.gpu_selection.pixel_bounds.is_none() {
                if let Some(ref data) = self.gpu_selection.cpu_cache {
                    self.gpu_selection.pixel_bounds = crate::mask::pixel_bounds_r8(
                        data,
                        self.gpu_selection.width,
                        self.gpu_selection.height,
                    );
                } else {
                    // Cache not ready — defer until SelectionReadback completes.
                    self.pending_transform = Some(PendingTransform {
                        layer_id,
                        target_is_mask,
                    });
                    return false;
                }
            }
            let [bx, by, bw, bh] = match self.gpu_selection.pixel_bounds {
                Some(b) => b,
                None => return false,
            };

            let x = (bx as i32).max(0);
            let y = (by as i32).max(0);
            let w = bw.min(canvas_w.saturating_sub(x as u32));
            let h = bh.min(canvas_h.saturating_sub(y as u32));

            if w == 0 || h == 0 {
                return false;
            }

            self.setup_transform(layer_id, target_is_mask, (x, y), w, h);
            true
        } else {
            // No selection — use compositor content bounds.
            if let Some(bounds) = self.compositor.content_bounds(layer_id) {
                // content_bounds are in layer-local coords; translate to
                // canvas-space via the layer texture's offset so callers
                // (and floating preview/uniforms) see canvas coords.
                let [bx, by, bw, bh] = bounds;
                if bw == 0 || bh == 0 {
                    return false;
                }
                let (off_x, off_y) = self
                    .compositor
                    .layer_texture(layer_id)
                    .map(|t| (t.offset_x, t.offset_y))
                    .unwrap_or((0, 0));
                let canvas_x = bx as i32 + off_x;
                let canvas_y = by as i32 + off_y;
                self.setup_transform(layer_id, target_is_mask, (canvas_x, canvas_y), bw, bh);
                true
            } else {
                // Bounds not yet computed — request async GPU compute.
                self.compositor.request_content_bounds(
                    &self.gpu.device,
                    &self.gpu.queue,
                    layer_id,
                    target_is_mask,
                );
                self.pending_transform = Some(super::PendingTransform {
                    layer_id,
                    target_is_mask,
                });
                false
            }
        }
    }

    /// Common setup logic for interactive transforms — saves pre-clear state,
    /// copies source region to floating texture, clears source on layer.
    pub(crate) fn setup_transform(
        &mut self,
        layer_id: u64,
        target_is_mask: bool,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
    ) {
        let format = if target_is_mask {
            wgpu::TextureFormat::R8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Look up the target layer's bounds so we can translate the
        // canvas-space `source_origin` into layer-local coords for any
        // operation that touches the layer texture directly (save_region,
        // restore_from_scratch, clear_rect on the layer).
        let (layer_off_x, layer_off_y, layer_w, layer_h) = if target_is_mask {
            self.compositor
                .mask_texture(layer_id)
                .map(|t| (t.offset_x, t.offset_y, t.width, t.height))
                .unwrap_or((0, 0, canvas_w, canvas_h))
        } else {
            self.compositor
                .layer_texture(layer_id)
                .map(|t| (t.offset_x, t.offset_y, t.width, t.height))
                .unwrap_or((0, 0, canvas_w, canvas_h))
        };

        // Layer-local rect of the source region — this is the slice of the
        // layer texture that the transform will modify, and the slice we
        // need to snapshot so cancel/undo can restore it.
        let local_x = (source_origin.0 - layer_off_x).max(0) as u32;
        let local_y = (source_origin.1 - layer_off_y).max(0) as u32;
        let local_w = source_width.min(layer_w.saturating_sub(local_x));
        let local_h = source_height.min(layer_h.saturating_sub(local_y));
        let layer_rect = [local_x, local_y, local_w, local_h];

        // Save the layer-local source region to scratch (pre-clear snapshot
        // for undo and cancel). Must happen before the clear.
        {
            let texture = if target_is_mask {
                self.compositor.mask_texture(layer_id).map(|t| &t.texture)
            } else {
                self.compositor.layer_texture(layer_id).map(|t| &t.texture)
            };
            if let Some(texture) = texture {
                // Source rect may exceed canvas bounds (paste-extent layer
                // transform). Pre-grow the scratch.
                self.region_store
                    .ensure_scratch_capacity(&self.gpu.device, layer_w, layer_h);
                self.gpu.encode("transform-save", |encoder| {
                    self.region_store
                        .save_region(encoder, texture, format, layer_rect);
                });
            }
        }

        // Copy source region from GPU texture to transform source texture.
        self.gpu.encode("transform-copy-source", |encoder| {
            self.compositor.set_floating_content_from_gpu(
                &self.gpu.device,
                &self.gpu.queue,
                encoder,
                source_origin,
                source_width,
                source_height,
                layer_id,
                target_is_mask,
            );
        });

        // If selection is active, mask the source texture so only selected pixels
        // are included in the transform. Also clear only selected pixels on the layer.
        let has_selection = self.gpu_selection.active;
        if has_selection {
            // Upload a cropped selection mask matching the source region dimensions.
            let cropped_sel_bg =
                self.upload_cropped_selection_r8(source_origin, source_width, source_height);
            // Full-canvas selection bind group from GPU selection.
            let full_sel_bg = Some(self.gpu_selection.paint_bind_group());

            if let Some(sel_bg) = &cropped_sel_bg {
                // Multiply source texture by selection mask — zeroes out unselected pixels.
                if let Some(source_tex) = self.compositor.transform_source_texture() {
                    let target = GpuPaintTarget {
                        texture: source_tex.0,
                        view: source_tex.1,
                        format,
                        width: source_width,
                        height: source_height,
                        offset_x: 0,
                        offset_y: 0,
                    };
                    self.gpu.encode("transform-sel-mask", |encoder| {
                        target.multiply_by_mask(
                            encoder,
                            &self.paint_pipelines,
                            &self.gpu.queue,
                            sel_bg,
                        );
                    });
                }
            }

            if let Some(sel_bg) = full_sel_bg {
                // Clear selected pixels on the layer using erase_with_selection.
                let layer_target = if target_is_mask {
                    self.compositor
                        .mask_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_mask(t, canvas_w, canvas_h))
                } else {
                    self.compositor
                        .layer_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_layer(t, canvas_w, canvas_h))
                };
                if let Some(target) = layer_target {
                    self.gpu.encode("transform-clear-sel", |encoder| {
                        target.erase_with_selection(
                            encoder,
                            &self.paint_pipelines,
                            &self.gpu.queue,
                            sel_bg,
                        );
                    });
                }
            }
        } else {
            // No selection — clear the layer-local source region on the layer.
            let target = if target_is_mask {
                self.compositor
                    .mask_texture(layer_id)
                    .map(|t| GpuPaintTarget::from_mask(t, canvas_w, canvas_h))
            } else {
                self.compositor
                    .layer_texture(layer_id)
                    .map(|t| GpuPaintTarget::from_layer(t, canvas_w, canvas_h))
            };
            if let Some(target) = target {
                self.gpu.encode("transform-clear", |encoder| {
                    target.clear_rect(encoder, &self.paint_pipelines, &self.gpu.queue, layer_rect);
                });
            }
        }

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            target_is_mask,
            // `clear_rect` is layer-local — `cancel_floating` uses it with
            // `restore_from_scratch` which copies scratch[rect] back to
            // texture[rect] in the layer texture's coord space.
            mode: FloatingMode::Transform {
                format,
                clear_rect: layer_rect,
            },
        });

        // Selection was used to define what gets picked up — clear it now so
        // the marching ants disappear and the transform output isn't clipped.
        if has_selection {
            self.gpu_selection.clear(&self.gpu.queue);

            self.selection_overlay.clear();
            self.push_merged_overlay();
        }
    }

    /// Update the floating content's transform matrix.
    pub fn update_floating_matrix(&mut self, matrix: Affine2D) {
        if let Some(fc) = &mut self.floating {
            fc.matrix = matrix;
            self.compositor.update_floating_matrix(
                &self.gpu.queue,
                &matrix,
                fc.source_origin,
                fc.source_width,
                fc.source_height,
            );
        }
    }

    /// Commit floating content: render transformed pixels into the target
    /// layer/mask texture via a GPU render pass.
    pub fn commit_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        let layer_id = fc.target_layer;
        let is_mask = fc.target_is_mask;
        let format = if is_mask {
            wgpu::TextureFormat::R8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };

        // Compute tight affected rect = union(source bounds, transformed bounds),
        // clamped to canvas.
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let (min_x, min_y, max_x, max_y) = fc.transformed_bounds();
        let (sox, soy) = fc.source_origin;
        let union_min_x = min_x.min(sox).max(0) as u32;
        let union_min_y = min_y.min(soy).max(0) as u32;
        let union_max_x = (max_x.max(sox + fc.source_width as i32) as u32).min(canvas_w);
        let union_max_y = (max_y.max(soy + fc.source_height as i32) as u32).min(canvas_h);
        let affected_w = union_max_x.saturating_sub(union_min_x);
        let affected_h = union_max_y.saturating_sub(union_min_y);
        let affected_rect = [union_min_x, union_min_y, affected_w, affected_h];

        // Path A — paste onto a layer auto-created for this paste.
        // The layer is empty by construction, so a single LayerAddAction
        // captures the whole paste as one undo step (no GpuRegionAction).
        if let FloatingMode::Paste {
            created_layer_id: Some(_),
        } = fc.mode
        {
            self.gpu.encode("paste-commit", |encoder| {
                self.compositor.commit_floating_to_texture(
                    &self.gpu.device,
                    encoder,
                    &self.gpu.queue,
                    &fc.matrix,
                    fc.source_origin,
                    fc.source_width,
                    fc.source_height,
                );
            });

            let parent = self.doc.parent_of(layer_id);
            let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);
            self.undo_stack
                .push(Box::new(LayerAddAction::new(layer_id, parent, pos)));

            self.compositor.clear_floating_content();
            return;
        }

        // Path B — paste-in-place onto an existing layer, or transform commit.
        // Paste-in-place hasn't called begin_transform, so save the pre-paste
        // canvas state now. Transform mode has already saved during setup.
        if matches!(fc.mode, FloatingMode::Paste { .. }) {
            let texture = if is_mask {
                self.compositor.mask_texture(layer_id).map(|t| &t.texture)
            } else {
                self.compositor.layer_texture(layer_id).map(|t| &t.texture)
            };
            if let Some(texture) = texture {
                self.gpu.encode("paste-save", |encoder| {
                    self.region_store.save_region(
                        encoder,
                        texture,
                        format,
                        [0, 0, canvas_w, canvas_h],
                    );
                });
            }
        }

        // Commit the pre-operation state (from scratch) to the undo ring buffer,
        // then render the transform.
        self.gpu.encode("transform-commit", |encoder| {
            let entry = self
                .region_store
                .commit_region(encoder, layer_id, format, affected_rect);

            // GPU render pass: write transformed source pixels to layer/mask texture.
            self.compositor.commit_floating_to_texture(
                &self.gpu.device,
                encoder,
                &self.gpu.queue,
                &fc.matrix,
                fc.source_origin,
                fc.source_width,
                fc.source_height,
            );

            // Push GPU undo action.
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });

        // Clean up GPU state
        self.compositor.clear_floating_content();
    }

    /// Cancel floating content: discard or restore original pixels.
    pub fn cancel_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        match fc.mode {
            FloatingMode::Paste { created_layer_id } => {
                // If the paste auto-created a target layer, drop it silently —
                // cancel restores the pre-paste document state. The layer's
                // pixels were never written (commit_floating_to_texture only
                // runs on commit), so just detach the node from the doc.
                // No undo entry: the LayerAddAction is only pushed on commit.
                if let Some(id) = created_layer_id {
                    self.doc.detach_for_undo(id);
                    self.compositor.mark_dirty();
                }
                // Otherwise: target layer was never modified — no-op.
            }
            FloatingMode::Transform { format, clear_rect } => {
                // Restore the pre-clear state from the RegionStore scratch
                // texture (saved during begin_transform).
                let texture = if fc.target_is_mask {
                    self.compositor
                        .mask_texture(fc.target_layer)
                        .map(|t| &t.texture)
                } else {
                    self.compositor
                        .layer_texture(fc.target_layer)
                        .map(|t| &t.texture)
                };
                if let Some(texture) = texture {
                    self.gpu.encode("cancel-restore", |encoder| {
                        self.region_store
                            .restore_from_scratch(encoder, format, clear_rect, texture);
                    });
                }
            }
        }

        self.compositor.clear_floating_content();
    }
}
