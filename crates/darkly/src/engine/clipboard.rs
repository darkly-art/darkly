//! Copy, cut, paste operations.

use super::{DarklyEngine, ReadbackContext};
use super::types::ClipboardExport;
use crate::clipboard::{Clipboard, ImageClip};
use crate::document::MoveTarget;
use crate::gpu::readback;
use crate::layer::Layer;
use crate::undo::LayerAddAction;

impl DarklyEngine {
    /// Copy the active layer's content (masked by selection) into the internal
    /// clipboard. Kicks off an async GPU readback — the result is available via
    /// `poll_copy_result()` on the next frame. Returns `None` immediately.
    pub fn copy(&mut self, layer_id: u64) -> Option<ClipboardExport> {
        if self.doc.layer(layer_id).is_none() {
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

    /// Start a GPU readback for copy (or cut). The readback completes
    /// asynchronously and is processed in `poll_pending`.
    fn start_copy_readback(&mut self, layer_id: u64, is_cut: bool) {
        let is_mask = self.editing_mask_layer == Some(layer_id);
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Determine bounds and check texture exists.
        let format = if is_mask {
            if self.compositor.mask_texture(layer_id).is_none() { return; }
            wgpu::TextureFormat::R8Unorm
        } else {
            if self.compositor.layer_texture(layer_id).is_none() { return; }
            wgpu::TextureFormat::Rgba8Unorm
        };
        let region = self.copy_region_from_selection(canvas_w, canvas_h);

        let texture = if is_mask {
            &self.compositor.mask_texture(layer_id).unwrap().texture
        } else {
            &self.compositor.layer_texture(layer_id).unwrap().texture
        };

        // Also readback the selection mask for the same region if present.
        let selection_data = self.readback_selection_region(region);

        self.gpu.encode("copy-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device, encoder, texture, format, region,
            );
            self.readbacks.submit(request, ReadbackContext::Copy {
                is_mask, region, selection_data, is_cut, layer_id,
            });
        });
    }

    /// Determine the copy region from the selection (or full canvas).
    fn copy_region_from_selection(&self, canvas_w: u32, canvas_h: u32) -> [u32; 4] {
        if let Some(sel) = &self.doc.selection {
            if let Some((tx_min, ty_min, tx_max, ty_max)) = sel.bounding_rect() {
                let ts = crate::tile::TILE_SIZE as i32;
                let x = (tx_min * ts).max(0) as u32;
                let y = (ty_min * ts).max(0) as u32;
                let w = (((tx_max - tx_min + 1) * ts) as u32).min(canvas_w.saturating_sub(x));
                let h = (((ty_max - ty_min + 1) * ts) as u32).min(canvas_h.saturating_sub(y));
                return [x, y, w, h];
            }
        }
        [0, 0, canvas_w, canvas_h]
    }

    /// Read selection coverage for a given region from CPU-side AlphaMask.
    /// Returns None if there's no selection.
    fn readback_selection_region(&self, region: [u32; 4]) -> Option<Vec<u8>> {
        let selection = self.doc.selection.as_ref()?;
        let [rx, ry, rw, rh] = region;
        Some(selection.rasterize_r8((rx as i32, ry as i32), rw, rh, 0))
    }

    /// Complete a pending copy once GPU readback data is available.
    pub(crate) fn complete_copy(
        &mut self, is_mask: bool, region: [u32; 4],
        selection_data: Option<Vec<u8>>, is_cut: bool, layer_id: u64, pixels: Vec<u8>,
    ) {
        let [rx, ry, rw, rh] = region;

        // Build RGBA bytes from the readback data.
        let (rgba, width, height) = if is_mask {
            // R8 readback → convert to grayscale RGBA: [v, v, v, 255]
            let mut rgba = vec![0u8; (rw * rh * 4) as usize];
            for i in 0..(rw * rh) as usize {
                let v = pixels[i];
                // Skip fully-revealed mask pixels (default state).
                if v == 255 && selection_data.is_none() {
                    // For masks, 255 = "reveal all" which is the default.
                    // Only include if selection forces inclusion.
                } else {
                    let sv = if let Some(ref sel) = selection_data {
                        let coverage = sel[i] as f32 / 255.0;
                        ((v as f32 * coverage).round()) as u8
                    } else {
                        v
                    };
                    if sv > 0 {
                        rgba[i * 4] = sv;
                        rgba[i * 4 + 1] = sv;
                        rgba[i * 4 + 2] = sv;
                        rgba[i * 4 + 3] = 255;
                    }
                }
            }
            (rgba, rw, rh)
        } else {
            // RGBA readback. Apply selection masking if present.
            let mut rgba = pixels;
            if let Some(ref sel) = selection_data {
                for i in 0..(rw * rh) as usize {
                    let coverage = sel[i] as f32 / 255.0;
                    if coverage <= 0.0 {
                        rgba[i * 4] = 0;
                        rgba[i * 4 + 1] = 0;
                        rgba[i * 4 + 2] = 0;
                        rgba[i * 4 + 3] = 0;
                    } else if coverage < 1.0 {
                        // Multiply alpha by selection coverage.
                        let a = rgba[i * 4 + 3] as f32 * coverage;
                        rgba[i * 4 + 3] = a.round() as u8;
                    }
                }
            }
            (rgba, rw, rh)
        };

        let offset_x = rx as i32;
        let offset_y = ry as i32;

        // Build ImageClip and store in clipboard.
        let clip = ImageClip::from_rgba(width, height, rgba, offset_x, offset_y);
        let (export_rgba, ew, eh, eox, eoy) = clip.to_rgba();
        self.pending_copy_result = Some(ClipboardExport {
            rgba: export_rgba.to_vec(),
            width: ew,
            height: eh,
            offset_x: eox,
            offset_y: eoy,
        });
        self.clipboard = Some(Clipboard::ImageData(clip));

        // If this was a cut, clear the source.
        if is_cut {
            if self.doc.selection.is_some() {
                self.gpu_clear_selection(layer_id);
            } else {
                self.gpu_clear_layer(layer_id);
            }
        }
    }

    /// Cut = copy + clear. The clear happens after the readback completes.
    /// Returns `None` immediately; result available via `poll_copy_result()`.
    pub fn cut(&mut self, layer_id: u64) -> Option<ClipboardExport> {
        if self.doc.layer(layer_id).is_none() {
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
        active_layer_id: Option<u64>,
    ) -> u64 {
        // Create a new layer and insert above the active layer.
        let id = self.doc.add_raster_layer();
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(id) {
            r.name = "Pasted Layer".to_string();
        }

        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);

        // Write RGBA data directly to the GPU layer texture.
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        // Clip the paste region to the canvas bounds.
        let src_x = (-offset_x).max(0) as u32;
        let src_y = (-offset_y).max(0) as u32;
        let dst_x = offset_x.max(0) as u32;
        let dst_y = offset_y.max(0) as u32;
        let copy_w = (width - src_x).min(canvas_w - dst_x);
        let copy_h = (height - src_y).min(canvas_h - dst_y);

        if copy_w > 0 && copy_h > 0 {
            if let Some(layer_tex) = self.compositor.layer_texture(id) {
                // Build a contiguous buffer for the clipped region.
                let row_bytes = copy_w as usize * 4;
                let mut buf = vec![0u8; row_bytes * copy_h as usize];
                for row in 0..copy_h as usize {
                    let src_row = (src_y as usize + row) * width as usize * 4 + src_x as usize * 4;
                    let dst_row = row * row_bytes;
                    buf[dst_row..dst_row + row_bytes]
                        .copy_from_slice(&rgba[src_row..src_row + row_bytes]);
                }

                self.gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &layer_tex.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: dst_x, y: dst_y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &buf,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes as u32),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: copy_w,
                        height: copy_h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        self.compositor.mark_dirty();

        // Position above active layer if specified.
        if let Some(active_id) = active_layer_id {
            self.doc.move_layer(id, MoveTarget::After(active_id));
        }

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Paste from the internal clipboard at its original position.
    /// Returns the new layer ID, or None if clipboard is empty.
    pub fn paste_in_place(&mut self, active_layer_id: Option<u64>) -> Option<u64> {
        let clip = self.clipboard.as_ref()?.as_image()?;
        let width = clip.width;
        let height = clip.height;
        let offset_x = clip.offset_x;
        let offset_y = clip.offset_y;
        let rgba = clip.data.clone();
        Some(self.paste_image(width, height, &rgba, offset_x, offset_y, active_layer_id))
    }
}
