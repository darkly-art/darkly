//! Rendering, view transform, thumbnails, undo/redo, and async readback polling.

use super::{DarklyEngine, ReadbackContext};
use crate::gpu::readback;
use crate::gpu::view::ViewTransform;
use crate::layer::BlendMode;

impl DarklyEngine {
    // --- View transform ---

    pub fn set_view_transform(
        &mut self,
        pan_x: f32, pan_y: f32,
        zoom: f32, rotation: f32,
        screen_w: f32, screen_h: f32,
    ) {
        let transform = ViewTransform::from_pan_zoom_rotate(
            pan_x, pan_y, zoom, rotation,
            screen_w, screen_h,
            self.doc.width as f32, self.doc.height as f32,
        );
        self.view_transform = transform;
        self.compositor.update_view_transform(&self.gpu.queue, &transform);
        self.compositor.mark_needs_present();
    }

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        self.view_transform.screen_to_canvas(screen_x, screen_y)
    }

    /// Start an async color pick at canvas coordinates.
    pub fn pick_color(&mut self, x: f32, y: f32) -> [u8; 4] {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let px = x as u32;
        let py = y as u32;

        if px >= canvas_w || py >= canvas_h {
            return [0, 0, 0, 0];
        }

        let texture = self.compositor.composited_texture();
        self.gpu.encode("pick-color", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device, encoder, texture,
                wgpu::TextureFormat::Rgba8Unorm, [px, py, 1, 1],
            );
            self.readbacks.submit(request, ReadbackContext::ColorPick);
        });

        // Return cached color for immediate feedback — real result arrives next frame.
        self.last_picked_color
    }

    // --- Thumbnails ---

    /// Return the cached layer thumbnail, kicking off an async readback if needed.
    pub fn layer_thumbnail(&mut self, layer_id: u64, thumb_w: u32, thumb_h: u32) -> Vec<u8> {
        let cached = self.thumbnail_cache.layer.get(&layer_id).cloned();
        self.request_thumbnail_readback(layer_id, false, thumb_w, thumb_h);
        cached.unwrap_or_else(|| vec![0u8; (thumb_w * thumb_h * 4) as usize])
    }

    /// Return the cached mask thumbnail, kicking off an async readback if needed.
    pub fn mask_thumbnail(&mut self, layer_id: u64, thumb_w: u32, thumb_h: u32) -> Vec<u8> {
        let has_mask = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().has_mask(),
            None => false,
        };
        if !has_mask {
            return Vec::new();
        }

        let cached = self.thumbnail_cache.mask.get(&layer_id).cloned();
        self.request_thumbnail_readback(layer_id, true, thumb_w, thumb_h);
        cached.unwrap_or_else(|| vec![0u8; (thumb_w * thumb_h * 4) as usize])
    }

    /// Kick off an async GPU readback for a thumbnail if one isn't already pending.
    fn request_thumbnail_readback(
        &mut self, layer_id: u64, is_mask: bool, thumb_w: u32, thumb_h: u32,
    ) {
        // Don't queue duplicate requests.
        if self.readbacks.any(|c| matches!(c, ReadbackContext::Thumbnail { layer_id: lid, is_mask: im, .. } if *lid == layer_id && *im == is_mask)) {
            return;
        }

        let doc_w = self.doc.width;
        let doc_h = self.doc.height;

        let (texture, format) = if is_mask {
            match self.compositor.mask_texture(layer_id) {
                Some(t) => (&t.texture, wgpu::TextureFormat::R8Unorm),
                None => return,
            }
        } else {
            match self.compositor.layer_texture(layer_id) {
                Some(t) => (&t.texture, wgpu::TextureFormat::Rgba8Unorm),
                None => return,
            }
        };

        self.gpu.encode("thumb-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device, encoder, texture, format, [0, 0, doc_w, doc_h],
            );
            self.readbacks.submit(request, ReadbackContext::Thumbnail {
                layer_id, is_mask, thumb_w, thumb_h,
            });
        });
    }


    // --- Rendering ---

    /// Poll all pending async readback operations.
    ///
    /// Called at the start of each frame. Returns true if any operation
    /// completed (and therefore the compositor should re-render).
    fn poll_pending(&mut self) -> bool {
        // Poll content bounds compute readbacks.
        let bounds_completed = self.compositor.poll_content_bounds(&self.gpu.device);
        let mut any_completed = false;

        // Complete pending transform if content bounds just arrived.
        if let Some(pt) = &self.pending_transform {
            if bounds_completed.contains(&pt.layer_id) {
                let layer_id = pt.layer_id;
                let target_is_mask = pt.target_is_mask;
                self.pending_transform = None;

                if self.floating.is_none() {
                    if let Some(bounds) = self.compositor.content_bounds(layer_id) {
                        let [bx, by, bw, bh] = bounds;
                        self.setup_transform(
                            layer_id, target_is_mask, (bx as i32, by as i32), bw, bh,
                        );
                        any_completed = true;
                    }
                }
            }
        }

        let completed = self.readbacks.poll(&self.gpu.device);
        if completed.is_empty() {
            return any_completed;
        }

        for (ctx, pixels) in completed {
            match ctx {
                ReadbackContext::FloodFill {
                    layer_id, mask_editing, seed_x, seed_y, color, tolerance,
                    canvas_w, canvas_h,
                } => self.complete_flood_fill(
                    layer_id, mask_editing, seed_x, seed_y, color, tolerance,
                    canvas_w, canvas_h, pixels,
                ),
                ReadbackContext::ColorPick => {
                    if pixels.len() >= 4 {
                        self.last_picked_color = [pixels[0], pixels[1], pixels[2], pixels[3]];
                    }
                }
                ReadbackContext::Copy { is_mask, region, selection_data, is_cut, layer_id } => {
                    self.complete_copy(is_mask, region, selection_data, is_cut, layer_id, pixels);
                }
                ReadbackContext::MagicWand { was_active, seed_x, seed_y, tolerance, mode } => {
                    self.complete_magic_wand(was_active, seed_x, seed_y, tolerance, mode, pixels);
                }
                ReadbackContext::MaskToSelection { was_active } => {
                    self.complete_mask_to_selection(was_active, pixels);
                }
                ReadbackContext::SelectionReadback => {
                    self.update_selection_overlay_from_readback(pixels);
                }
                ReadbackContext::Thumbnail { layer_id, is_mask, thumb_w, thumb_h } => {
                    let doc_w = self.doc.width;
                    let doc_h = self.doc.height;
                    if is_mask {
                        let thumb = generate_mask_thumbnail_from_pixels(
                            &pixels, doc_w, doc_h, thumb_w, thumb_h,
                        );
                        self.thumbnail_cache.mask.insert(layer_id, thumb);
                    } else {
                        let thumb = generate_rgba_thumbnail_from_pixels(
                            &pixels, doc_w, doc_h, thumb_w, thumb_h,
                        );
                        self.thumbnail_cache.layer.insert(layer_id, thumb);
                    }
                }
            }
        }
        true
    }

    /// Get the most recently picked color (updated asynchronously).
    pub fn last_picked_color(&self) -> [u8; 4] {
        self.last_picked_color
    }

    /// True if a color pick readback is still in flight.
    pub fn has_pending_color_pick(&self) -> bool {
        self.readbacks.any(|c| matches!(c, ReadbackContext::ColorPick))
    }

    /// Render a frame. Returns true if animations need another frame.
    pub fn render(&mut self, time_secs: f32) -> bool {
        let pending_completed = self.poll_pending();
        if pending_completed {
            self.compositor.mark_dirty();
        }

        // Headless mode (tests): poll pending ops but skip presentation.
        let (surface, surface_config) = match (&self.gpu.surface, &self.gpu.surface_config) {
            (Some(s), Some(c)) => (s, c),
            _ => {
                return self.readbacks.has_pending()
                    || self.compositor.has_pending_content_bounds();
            }
        };

        // Skip rendering when the surface has zero dimensions (e.g. canvas
        // squeezed to 0 height by a UI panel).  WebGPU cannot create
        // 0-dimension textures and attempting to do so corrupts the device.
        if surface_config.width == 0 || surface_config.height == 0 {
            return self.readbacks.has_pending()
                || self.compositor.has_pending_content_bounds();
        }

        self.compositor.update_animations(&self.gpu.queue, time_secs);
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            surface,
            surface_config,
            &mut self.doc,
        );

        // Keep requesting frames while async operations are in flight.
        self.compositor.needs_animation()
            || self.readbacks.has_pending()
            || self.compositor.has_pending_content_bounds()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || self.gpu.is_headless() {
            return;
        }
        self.gpu.resize(width, height);
        self.compositor.veil_chain_mut().resize(&self.gpu.device, &self.gpu.queue, width, height);
        self.compositor.mark_needs_present();
    }

    // --- Undo / Redo ---

    pub fn undo(&mut self) {
        self.auto_commit_floating();
        self.apply_undo(UndoDirection::Undo);
    }

    pub fn redo(&mut self) {
        self.auto_commit_floating();
        self.apply_undo(UndoDirection::Redo);
    }

    fn apply_undo(&mut self, direction: UndoDirection) {
        let action = match direction {
            UndoDirection::Undo => self.undo_stack.pop_for_undo(),
            UndoDirection::Redo => self.undo_stack.pop_for_redo(),
        };
        let mut action = match action {
            Some(a) => a,
            None => return,
        };

        match direction {
            UndoDirection::Undo => { action.undo(&mut self.doc); },
            UndoDirection::Redo => { action.redo(&mut self.doc); },
        }

        // Sync layer/mask state BEFORE restoring GPU regions, so that mask
        // textures are (re)created if needed by the undo action.
        self.sync_compositor_layers();

        // If this is a GPU region action, execute the texture restore.
        if let Some(entry) = action.gpu_region_entry_mut() {
            let texture = if entry.format == wgpu::TextureFormat::R8Unorm {
                self.compositor.mask_texture(entry.layer_id).map(|t| &t.texture)
            } else {
                self.compositor.layer_texture(entry.layer_id).map(|t| &t.texture)
            };
            if let Some(texture) = texture {
                self.gpu.encode(match direction {
                    UndoDirection::Undo => "undo-restore",
                    UndoDirection::Redo => "redo-restore",
                }, |encoder| {
                    let swapped = self.region_store.restore_region(encoder, entry, texture);
                    *entry = swapped;
                });
            }
        }

        // If this is a selection GPU action, restore the selection texture
        // and swap the active flag.
        if let Some(restored_active) = action.swap_selection_active(self.gpu_selection.active) {
            self.gpu_selection.active = restored_active;

            if let Some(entry) = action.selection_region_entry_mut() {
                let texture = self.gpu_selection.texture();
                self.gpu.encode(match direction {
                    UndoDirection::Undo => "undo-sel-restore",
                    UndoDirection::Redo => "redo-sel-restore",
                }, |encoder| {
                    let swapped = self.region_store.restore_region(encoder, entry, texture);
                    *entry = swapped;
                });
            }

            self.gpu_selection.pixel_bounds = None; // will be recomputed from readback
            self.kick_selection_readback();
        }

        match direction {
            UndoDirection::Undo => self.undo_stack.complete_undo(action),
            UndoDirection::Redo => self.undo_stack.complete_redo(action),
        }
        self.compositor.mark_dirty();
    }

    // --- Internal helpers ---

    pub(crate) fn sync_compositor_layers(&mut self) {
        // Collect raster layer info first to avoid borrow conflicts with mask_dirty.
        struct RasterInfo {
            id: u64,
            opacity: f32,
            blend_mode: BlendMode,
            show_mask: bool,
            mask_enabled: bool,
            has_mask: bool,
        }
        let infos: Vec<RasterInfo> = self.doc.all_raster_layers().into_iter().map(|r| {
            RasterInfo {
                id: r.id, opacity: r.opacity, blend_mode: r.blend_mode,
                show_mask: r.show_mask, mask_enabled: r.mask_enabled,
                has_mask: r.has_mask,
            }
        }).collect();

        for info in &infos {
            self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, info.id);
            self.compositor.update_raster_uniforms_full(
                &self.gpu.queue, info.id, info.opacity, info.blend_mode, info.show_mask,
            );
            self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, info.id, info.has_mask);
            self.compositor.update_mask_binding(
                &self.gpu.device, info.id, info.mask_enabled, info.show_mask,
            );
        }

        // Sync non-passthrough group state
        let groups: Vec<(u64, f32, BlendMode, bool)> = self.doc.all_groups()
            .iter()
            .filter(|g| !g.passthrough)
            .map(|g| (g.id, g.opacity, g.blend_mode, g.show_mask))
            .collect();
        for (id, opacity, blend_mode, show_mask) in groups {
            self.compositor.ensure_group_state(&self.gpu.device, &self.gpu.queue, id);
            self.compositor.update_group_uniforms(&self.gpu.queue, id, opacity, blend_mode, show_mask);
        }
    }
}

enum UndoDirection { Undo, Redo }

// ---------------------------------------------------------------------------
// Thumbnail generation — nearest-neighbor sampling from GPU readback pixels
// ---------------------------------------------------------------------------

fn generate_rgba_thumbnail_from_pixels(
    pixels: &[u8],
    doc_w: u32, doc_h: u32,
    thumb_w: u32, thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);

            let src = ((cy * doc_w + cx) * 4) as usize;
            let (r, g, b, a) = if src + 3 < pixels.len() {
                (pixels[src], pixels[src + 1], pixels[src + 2], pixels[src + 3])
            } else {
                (0, 0, 0, 0)
            };

            let off = ((oy * thumb_w + ox) * 4) as usize;
            // Checkerboard behind transparent areas
            let check = if ((ox / 4) + (oy / 4)) % 2 == 0 { 102u8 } else { 153u8 };
            let af = a as f32 / 255.0;
            buf[off]     = (r as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 1] = (g as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 2] = (b as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 3] = 255;
        }
    }
    buf
}

fn generate_mask_thumbnail_from_pixels(
    pixels: &[u8],
    doc_w: u32, doc_h: u32,
    thumb_w: u32, thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);

            let v = pixels.get((cy * doc_w + cx) as usize).copied().unwrap_or(255);

            let off = ((oy * thumb_w + ox) * 4) as usize;
            buf[off]     = v;
            buf[off + 1] = v;
            buf[off + 2] = v;
            buf[off + 3] = 255;
        }
    }
    buf
}
