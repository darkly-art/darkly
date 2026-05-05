//! Copy, cut, paste operations.

use super::types::ClipboardExport;
use super::{DarklyEngine, PendingCopy, ReadbackContext};
use crate::clipboard::{Clipboard, ImageClip};
use crate::document::MoveTarget;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback;
use crate::layer::{Layer, LayerId};
use crate::undo::{GpuRegionAction, LayerAddAction};

impl DarklyEngine {
    /// Copy the active layer's content (masked by selection) into the internal
    /// clipboard. Kicks off an async GPU readback — the result is available via
    /// `poll_copy_result()` on the next frame. Returns `None` immediately.
    pub fn copy(&mut self, layer_id: LayerId) -> Option<ClipboardExport> {
        self.doc.layer(layer_id)?;

        if self.has_selection() && self.selection_cpu_cache().is_none() {
            // Selection cache not ready — defer until SelectionReadback completes.
            self.pending_copy = Some(PendingCopy {
                layer_id,
                is_cut: false,
            });
            return None;
        }

        self.start_copy_readback(layer_id, false);
        None
    }

    /// Poll for a completed copy result. Returns the ClipboardExport once the
    /// GPU readback has completed (typically the next frame after copy/cut).
    pub fn poll_copy_result(&mut self) -> Option<ClipboardExport> {
        self.pending_copy_result.take()
    }

    /// Start a GPU readback for copy (or cut).
    ///
    /// When a selection is active, masking is done entirely on GPU:
    /// 1. Copy the layer region to a staging texture
    /// 2. Multiply staging by the cropped selection mask (`layer * sel`)
    /// 3. Async readback the staging texture (pre-masked pixels for clipboard)
    /// 4. If cut: erase selected pixels on the layer (`layer *= (1 - sel)`)
    ///
    /// Both extraction and erase use GPU float math on the same selection
    /// texture, guaranteeing `extracted + remaining == original`.
    pub(crate) fn start_copy_readback(&mut self, layer_id: LayerId, is_cut: bool) {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Determine format from the unified node-texture pool — both raster
        // (RGBA8) and mask modifier (R8) targets resolve through the same
        // call. Caller's id alone selects the surface; format follows.
        let format = match self.compositor.node_texture(layer_id) {
            Some(t) => t.format,
            None => return,
        };

        // Compute copy region from selection bounds (or full canvas).
        let region = match self.copy_region_from_selection(canvas_w, canvas_h) {
            Some(r) => r,
            None => {
                // Selection bounds unknown — defer.
                self.pending_copy = Some(PendingCopy { layer_id, is_cut });
                return;
            }
        };
        let [rx, ry, rw, rh] = region;
        if rw == 0 || rh == 0 {
            return;
        }

        let has_selection = self.has_selection();

        // Resolve the node's CanvasFrame once for both the cut undo save
        // below and the matching commit further down. Format dispatch lives
        // behind the unified node-texture pool.
        let target_frame = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.canvas_frame());
        let undo_rect = target_frame
            .map(|f| f.canvas_extent)
            .unwrap_or_else(|| crate::coord::CanvasRect::from_xywh(0, 0, canvas_w, canvas_h));

        if has_selection {
            // --- GPU extraction path ---
            // Save undo state for cut before any modification.
            let cut_snapshot = if is_cut {
                target_frame.map(|frame| {
                    self.gpu.encode_ret("cut-save", |encoder| {
                        self.region_store
                            .save_region(encoder, &frame, format, undo_rect)
                    })
                })
            } else {
                None
            };

            // Create staging texture for the masked copy.
            let staging_tex = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("copy-staging"),
                size: wgpu::Extent3d {
                    width: rw,
                    height: rh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let staging_view = staging_tex.create_view(&wgpu::TextureViewDescriptor::default());

            // Create a cropped selection R8 texture for the copy region.
            let sel_crop_tex = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("copy-sel-crop"),
                size: wgpu::Extent3d {
                    width: rw,
                    height: rh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let sel_crop_view = sel_crop_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let sel_sampler = self.gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("copy-sel-sampler"),
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            let sel_crop_bg = self.paint_pipelines.create_selection_bind_group(
                &self.gpu.device,
                &sel_crop_view,
                &sel_sampler,
            );

            // Get texture references before entering the encode closure.
            let layer_tex = &self.compositor.node_texture(layer_id).unwrap().texture;
            let selection_state = self
                .compositor
                .selection_state()
                .expect("has_selection true → selection_state allocated");
            let sel_tex = selection_state.texture();
            let sel_paint_bg = selection_state.paint_bind_group();

            // Compute overlap for selection crop (selection and layer are same canvas size).
            let sel_copy_w = rw.min(self.doc.width.saturating_sub(rx));
            let sel_copy_h = rh.min(self.doc.height.saturating_sub(ry));

            self.gpu.encode("copy-gpu-extract", |encoder| {
                // 1. Copy layer region → staging texture.
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: layer_tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: rx, y: ry, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &staging_tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: rw,
                        height: rh,
                        depth_or_array_layers: 1,
                    },
                );

                // 2. Copy selection region → cropped selection texture.
                if sel_copy_w > 0 && sel_copy_h > 0 {
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: sel_tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x: rx, y: ry, z: 0 },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: &sel_crop_tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: sel_copy_w,
                            height: sel_copy_h,
                            depth_or_array_layers: 1,
                        },
                    );
                }

                // 3. Multiply staging alpha by cropped selection: staging.a *= sel.
                //    RGB stays unchanged — straight-alpha convention (see
                //    compositing-lessons-learned.md §1).
                let staging_target = GpuPaintTarget {
                    texture: &staging_tex,
                    view: &staging_view,
                    format,
                    width: rw,
                    height: rh,
                    offset_x: 0,
                    offset_y: 0,
                    canvas_width: rw,
                    canvas_height: rh,
                };
                staging_target.multiply_alpha_by_mask(
                    encoder,
                    &self.paint_pipelines,
                    &self.gpu.queue,
                    &sel_crop_bg,
                );

                // 4. If cut: reduce layer alpha by selection on selected pixels.
                //    layer.a *= (1 - sel), layer.rgb unchanged (straight alpha).
                if is_cut {
                    let layer_target = self
                        .compositor
                        .node_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_node(t, canvas_w, canvas_h))
                        .expect("node texture missing for cut target");
                    layer_target.multiply_alpha_by_inverse_mask(
                        encoder,
                        &self.paint_pipelines,
                        &self.gpu.queue,
                        sel_paint_bg,
                    );
                }

                // 5. Kick async readback of the masked staging texture.
                let request = readback::request_readback(
                    &self.gpu.device,
                    encoder,
                    &staging_tex,
                    format,
                    [0, 0, rw, rh],
                );
                self.readbacks.submit(
                    request,
                    ReadbackContext::Copy {
                        node_id: layer_id,
                        region,
                        is_cut,
                    },
                );
            });

            // Commit undo for cut.
            if let (Some(snap), Some(frame)) = (cut_snapshot, target_frame) {
                self.gpu.encode("cut-commit", |encoder| {
                    let entry = self
                        .region_store
                        .commit_region(encoder, layer_id, &frame, &snap, undo_rect);
                    self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
                });
                self.compositor.mark_node_pixels_dirty(layer_id);
            }
        } else {
            // --- No selection: direct readback ---
            let texture = self
                .compositor
                .node_texture(layer_id)
                .map(|t| &t.texture)
                .expect("node texture missing for copy");

            self.gpu.encode("copy-readback", |encoder| {
                let request =
                    readback::request_readback(&self.gpu.device, encoder, texture, format, region);
                self.readbacks.submit(
                    request,
                    ReadbackContext::Copy {
                        node_id: layer_id,
                        region,
                        is_cut,
                    },
                );
            });

            if is_cut {
                // gpu_clear_layer handles its own undo save/commit.
                self.gpu_clear_layer(layer_id);
            }
        }
    }

    /// Determine the copy region from the selection (or full canvas).
    /// Returns `None` if the selection is active but bounds are unknown
    /// (cache not yet populated from async readback).
    fn copy_region_from_selection(&mut self, canvas_w: u32, canvas_h: u32) -> Option<[u32; 4]> {
        if !self.has_selection() {
            return Some([0, 0, canvas_w, canvas_h]);
        }
        if self.selection_pixel_bounds().is_none() {
            // Recompute bounds from the CPU cache (populated by the async
            // SelectionReadback after every mutating op).
            let bounds = {
                let data = self.selection_cpu_cache()?;
                crate::mask::pixel_bounds_r8(data, self.doc.width, self.doc.height).map(
                    |[x, y, w, h]| crate::coord::CanvasRect::from_xywh(x as i32, y as i32, w, h),
                )
            };
            self.set_selection_pixel_bounds(bounds);
        }
        if let Some(bounds) = self.selection_pixel_bounds() {
            let x = bounds.x0().max(0) as u32;
            let y = bounds.y0().max(0) as u32;
            let w = bounds.width.min(canvas_w.saturating_sub(x));
            let h = bounds.height.min(canvas_h.saturating_sub(y));
            Some([x, y, w, h])
        } else {
            Some([0, 0, canvas_w, canvas_h])
        }
    }

    /// Complete a pending copy once GPU readback data is available.
    /// Pixels arrive pre-masked from the GPU staging texture (when selection
    /// was active) or raw from the layer (when no selection).
    pub(crate) fn complete_copy(
        &mut self,
        node_id: LayerId,
        region: [u32; 4],
        is_cut: bool,
        pixels: Vec<u8>,
    ) {
        let [rx, ry, rw, rh] = region;

        // Build RGBA bytes from the readback data — format dispatch is now
        // driven by the source node's texture format, not a sidecar boolean.
        let is_r8 = self
            .compositor
            .node_texture(node_id)
            .map(|t| t.format == wgpu::TextureFormat::R8Unorm)
            .unwrap_or(false);
        let rgba = if is_r8 {
            // R8 readback → convert to grayscale RGBA: [v, v, v, 255]
            let mut rgba = vec![0u8; (rw * rh * 4) as usize];
            for i in 0..(rw * rh) as usize {
                let v = pixels[i];
                if v > 0 {
                    rgba[i * 4] = v;
                    rgba[i * 4 + 1] = v;
                    rgba[i * 4 + 2] = v;
                    rgba[i * 4 + 3] = 255;
                }
            }
            rgba
        } else {
            // RGBA readback — already masked by GPU if selection was active.
            pixels
        };

        let clip = ImageClip::from_rgba(rw, rh, rgba, rx as i32, ry as i32);
        let (export_rgba, ew, eh, eox, eoy) = clip.to_rgba();
        self.pending_copy_result = Some(ClipboardExport {
            rgba: export_rgba.to_vec(),
            width: ew,
            height: eh,
            offset_x: eox,
            offset_y: eoy,
        });
        self.clipboard = Some(Clipboard::ImageData(clip));

        let _ = is_cut;
    }

    /// Cut = copy + clear. The clear happens on GPU during start_copy_readback.
    /// Returns `None` immediately; result available via `poll_copy_result()`.
    pub fn cut(&mut self, layer_id: LayerId) -> Option<ClipboardExport> {
        self.doc.layer(layer_id)?;

        if self.has_selection() && self.selection_cpu_cache().is_none() {
            self.pending_copy = Some(PendingCopy {
                layer_id,
                is_cut: true,
            });
            return None;
        }

        self.start_copy_readback(layer_id, true);
        None
    }

    /// Paste raw RGBA bytes as a new layer. Used for both internal and external
    /// clipboard content. Returns the new layer ID.
    pub fn paste_image(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: Option<LayerId>,
    ) -> LayerId {
        // Size the new layer to fit the paste exactly, so out-of-canvas
        // pixels are preserved.
        let layer_bounds = crate::coord::CanvasRect::from_xywh(offset_x, offset_y, width, height);

        // Create a new layer and insert above the active layer.
        let id = self.doc.add_raster_layer(None);
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(id) {
            r.common.name = "Pasted Layer".to_string();
            r.pixels.bounds = layer_bounds;
        }

        self.compositor
            .ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id, layer_bounds);

        // Upload the entire RGBA buffer to the layer texture — its bounds
        // exactly match the paste extent so no per-row copy or clipping is
        // needed.
        if let Some(layer_tex) = self.compositor.node_texture(id) {
            self.gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &layer_tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.compositor.mark_node_pixels_dirty(id);

        // Position above active layer if specified.
        if let Some(active_id) = active_layer_id {
            self.doc.move_layer(id, MoveTarget::After(active_id));
        }

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack
            .push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Paste from the internal clipboard at its original position.
    /// Returns the new layer ID, or None if clipboard is empty.
    pub fn paste_in_place(&mut self, active_layer_id: Option<LayerId>) -> Option<LayerId> {
        let clip = self.clipboard.as_ref()?.as_image()?;
        let width = clip.width;
        let height = clip.height;
        let offset_x = clip.offset_x;
        let offset_y = clip.offset_y;
        let rgba = clip.data.clone();
        Some(self.paste_image(width, height, &rgba, offset_x, offset_y, active_layer_id))
    }
}
