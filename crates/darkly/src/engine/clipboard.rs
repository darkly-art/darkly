//! Copy, cut, paste operations.

use darkly_macros::handlers;

use super::rendering::commit_undo_region;
use super::types::ClipboardExport;
use super::{DarklyEngine, PendingCopy, ReadbackContext, RichCopyMask, RichCopyMetadata};
use crate::clipboard::{
    Clipboard, ClipboardRect, ImageClip, LayerClipboard, MaskClipboard,
    LAYER_CLIPBOARD_SCHEMA_VERSION,
};
use crate::gpu::blend_mode;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback;
use crate::layer::{Layer, LayerId};
use crate::undo::{GpuRegionAction, LayerAddAction};

#[handlers]
impl DarklyEngine {
    /// Copy the active layer's content (masked by selection) into the internal
    /// clipboard. Kicks off an async GPU readback — the result is available via
    /// `poll_copy_result()` on the next frame. Returns `None` immediately.
    #[handler]
    pub fn copy(&mut self, id: LayerId) -> Option<ClipboardExport> {
        // The copy target is any pixel-bearing node: a raster/void layer, or a
        // filter (the active paint target when editing a mask is the mask
        // filter id directly). `start_copy_readback` gates on the node's
        // texture, so a non-pixel target simply no-ops there.
        if self.doc.layer(id).is_none() && !self.doc.is_filter(id) {
            return None;
        }

        if self.has_selection() && self.selection_cpu_cache().is_none() {
            // Selection cache not ready — defer until SelectionReadback completes.
            self.pending_copy = Some(PendingCopy {
                layer_id: id,
                is_cut: false,
            });
            return None;
        }

        self.start_copy_readback(id, false);
        None
    }

    /// Poll for a completed copy result. Returns the ClipboardExport once the
    /// GPU readback has completed (typically the next frame after copy/cut).
    #[handler]
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
        // (RGBA8) and mask filter (R8) targets resolve through the same
        // call. Caller's id alone selects the surface; format follows.
        let format = match self.compositor.node_texture(layer_id) {
            Some(t) => t.format(),
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
        let [_, _, rw, rh] = region;
        if rw == 0 || rh == 0 {
            return;
        }

        if self.has_selection() {
            self.copy_with_selection(layer_id, region, is_cut, format);
        } else {
            self.copy_without_selection(layer_id, region, is_cut, format);
        }
    }

    /// Copy/cut path when a selection is active: mask the layer region by the
    /// selection entirely on GPU, then async-read the pre-masked staging
    /// texture. On cut, the same selection texture erases the layer in place,
    /// guaranteeing `extracted + remaining == original`.
    fn copy_with_selection(
        &mut self,
        layer_id: LayerId,
        region: [u32; 4],
        is_cut: bool,
        format: wgpu::TextureFormat,
    ) {
        let [rx, ry, rw, rh] = region;
        // The copy `region` is window-local; layer reads operate in plane space,
        // so they lift it by `canvas_origin` (identity for an un-cropped doc).
        let canvas_origin = self.doc.canvas_origin;

        // Resolve the node's CanvasFrame once for both the cut undo save below
        // and the matching commit further down.
        let target_frame = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.canvas_frame());
        let undo_rect = target_frame
            .map(|f| f.canvas_extent)
            .unwrap_or_else(|| self.doc.canvas_rect());

        // Save undo state for cut before any modification.
        let cut_snapshot = if is_cut {
            target_frame.map(|frame| {
                self.gpu.encode_ret("cut-save", |encoder| {
                    self.region_scratch.save_region(
                        &self.gpu.device,
                        encoder,
                        &frame,
                        format,
                        undo_rect,
                    )
                })
            })
        } else {
            None
        };

        let (staging_tex, staging_view) = crate::gpu::create_texture_with_view(
            &self.gpu.device,
            rw,
            rh,
            format,
            "copy-staging",
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
        );

        // Crop the live selection to the copy region (nearest — pixel-exact).
        let region_window = crate::coord::WindowRect::from_xywh(rx as i32, ry as i32, rw, rh);
        let sel_crop_bg = self
            .selection_region_bind_group(region_window, wgpu::FilterMode::Nearest)
            .expect("has_selection true → selection_state allocated");

        // Get texture references before entering the encode closure.
        let layer_node = self.compositor.node_texture(layer_id).unwrap();
        let layer_tex = layer_node.texture();
        // The selection region in PLANE space, and the portion of it the layer
        // texture actually holds, translated to layer-local texels. The layer is
        // plane-anchored at its own extent (which need not be the canvas
        // window), so the copy source must come through `canvas_to_layer_rect`,
        // not the raw window-local `(rx, ry)`.
        let region_plane = region_window.to_canvas(canvas_origin);
        let layer_copy = layer_node
            .canvas_extent()
            .intersect(region_plane)
            .and_then(|ov| layer_node.canvas_to_layer_rect(ov).map(|lr| (ov, lr)));

        let sel_paint_bg = self
            .compositor
            .selection_state()
            .expect("has_selection true → selection_state allocated")
            .selection_bind_group();

        let mut encoder = crate::gpu::paint_target::PaintCommandEncoder::new(
            &self.gpu.device,
            &self.gpu.queue,
            &self.paint_pipelines,
            "copy-gpu-extract",
            if is_cut { 2 } else { 1 },
        );
        encoder.with_raw(|raw| {
            // 1. Copy the layer's covered portion of the selected region into
            //    the staging texture. Clear first so any part of the region the
            //    layer does NOT cover stays transparent (else the masked
            //    multiply below would scale uninitialized texels).
            crate::gpu::clear_view_transparent(raw, &staging_view, "copy-staging-clear");
            if let Some((ov, lr)) = layer_copy {
                // Staging is indexed by the selection region, so the destination
                // is the overlap's offset within it.
                crate::gpu::blit_region(
                    raw,
                    layer_tex,
                    (lr.x0(), lr.y0()),
                    &staging_tex,
                    (
                        (ov.x0() - region_plane.x0()) as u32,
                        (ov.y0() - region_plane.y0()) as u32,
                    ),
                    ov.width,
                    ov.height,
                );
            }
        });

        // 2. Multiply staging alpha by cropped selection: staging.a *= sel.
        //    RGB stays unchanged — straight-alpha convention (see
        //    docs/lessons-learned/compositing-lessons-learned.md §1).
        let staging_target = GpuPaintTarget::from_canvas_texture(
            &staging_tex,
            &staging_view,
            format,
            crate::coord::CanvasRect::from_xywh(0, 0, rw, rh),
        );
        staging_target.multiply_alpha_by_mask(
            &mut encoder,
            &self.paint_pipelines,
            &self.gpu.queue,
            &sel_crop_bg,
        );

        // 3. If cut: reduce layer alpha by selection on selected pixels.
        //    layer.a *= (1 - sel), layer.rgb unchanged (straight alpha).
        if is_cut {
            let layer_target = self
                .compositor
                .node_texture(layer_id)
                .map(|t| GpuPaintTarget::from_node(t, self.doc.canvas_rect()))
                .expect("node texture missing for cut target");
            layer_target.multiply_alpha_by_inverse_mask(
                &mut encoder,
                &self.paint_pipelines,
                &self.gpu.queue,
                sel_paint_bg,
            );
        }

        // 4. Kick async readback of the masked staging texture.
        let request = encoder.with_raw(|raw| {
            readback::request_readback(
                &self.gpu.device,
                raw,
                &staging_tex,
                format,
                crate::coord::LayerRect::from_xywh(0, 0, rw, rh),
            )
        });
        self.readbacks.submit(
            request,
            ReadbackContext::Copy {
                node_id: layer_id,
                region,
                is_cut,
            },
        );
        encoder.submit();

        // Commit undo for cut.
        if let (Some(snap), Some(frame)) = (cut_snapshot, target_frame) {
            let entry = commit_undo_region(
                &self.gpu,
                &self.region_scratch,
                &mut self.readbacks,
                "cut-commit",
                layer_id,
                &frame,
                &snap,
                undo_rect,
            );
            self.push_undo(Box::new(GpuRegionAction::new(entry)));
            self.compositor.mark_node_pixels_dirty(layer_id);
        }
    }

    /// Copy/cut path with no selection: read the layer's covered portion of the
    /// requested region straight back, no masking. On cut, clear the whole
    /// layer (which handles its own undo).
    fn copy_without_selection(
        &mut self,
        layer_id: LayerId,
        region: [u32; 4],
        is_cut: bool,
        format: wgpu::TextureFormat,
    ) {
        let [rx, ry, rw, rh] = region;
        // `region` is the canvas window in window-local coords; lift it to plane
        // (`+ canvas_origin`), then translate to the layer texture's own texel
        // frame (it may sit at a non-zero plane offset). Layers disjoint from the
        // requested region yield nothing.
        let (texture, layer_rect) = {
            let layer_tex = self
                .compositor
                .node_texture(layer_id)
                .expect("node texture missing for copy");
            let region_plane = crate::coord::WindowRect::from_xywh(rx as i32, ry as i32, rw, rh)
                .to_canvas(self.doc.canvas_origin);
            match layer_tex.canvas_to_layer_rect(region_plane) {
                Some(lr) => (layer_tex.texture(), lr),
                None => return,
            }
        };

        self.gpu.encode("copy-readback", |encoder| {
            let request =
                readback::request_readback(&self.gpu.device, encoder, texture, format, layer_rect);
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
                    |[x, y, w, h]| crate::coord::WindowRect::from_xywh(x as i32, y as i32, w, h),
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
            .map(|t| t.format() == wgpu::TextureFormat::R8Unorm)
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

        // `region`'s `(rx, ry)` is window-local; the clipboard offset is a plane
        // position (paste-in-place re-creates the layer at `from_xywh(offset, …)`
        // in plane space), so lift it by `canvas_origin`.
        let o = self.doc.canvas_origin;
        let clip = ImageClip::from_rgba(rw, rh, rgba, rx as i32 + o.x, ry as i32 + o.y);
        let (export_rgba, ew, eh, eox, eoy) = clip.to_rgba();
        self.pending_copy_result = Some(ClipboardExport {
            rgba: export_rgba.to_vec(),
            width: ew,
            height: eh,
            offset_x: eox,
            offset_y: eoy,
        });

        // Promote to a `LayerClipboard` if `copy_layer_rich` captured the
        // source layer's metadata. The pixels we just received become the
        // base64 payload; everything else (blend mode, opacity, …) was
        // snapshotted at copy time.
        if let Some(meta) = self.pending_rich_metadata.take() {
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let layer_clip = LayerClipboard {
                schema_version: LAYER_CLIPBOARD_SCHEMA_VERSION,
                name: meta.name,
                visible: meta.visible,
                locked: meta.locked,
                opacity: meta.opacity,
                blend_mode: meta.blend_mode,
                bounds: ClipboardRect {
                    x: eox,
                    y: eoy,
                    width: ew,
                    height: eh,
                },
                pixels_b64: STANDARD.encode(export_rgba),
                mask: meta.mask.map(|m| MaskClipboard {
                    name: m.name,
                    visible: m.visible,
                    bounds: ClipboardRect {
                        x: m.bounds.x0(),
                        y: m.bounds.y0(),
                        width: m.bounds.width,
                        height: m.bounds.height,
                    },
                    pixels_b64: String::new(),
                }),
            };
            self.pending_layer_clip = Some(layer_clip.to_json());
            self.clipboard = Some(Clipboard::Layer(layer_clip));
        } else {
            self.clipboard = Some(Clipboard::ImageData(clip));
        }

        let _ = is_cut;
    }

    /// Cut = copy + clear. The clear happens on GPU during start_copy_readback.
    /// Returns `None` immediately; result available via `poll_copy_result()`.
    #[handler]
    pub fn cut(&mut self, id: LayerId) -> Option<ClipboardExport> {
        // Accept a filter id (mask edit target) as well as a layer — see
        // `copy`. The actual texture gate lives in `start_copy_readback`.
        if self.doc.layer(id).is_none() && !self.doc.is_filter(id) {
            return None;
        }
        if !self.doc.is_node_editable(id) {
            return None;
        }

        if self.has_selection() && self.selection_cpu_cache().is_none() {
            self.pending_copy = Some(PendingCopy {
                layer_id: id,
                is_cut: true,
            });
            return None;
        }

        self.start_copy_readback(id, true);
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

        // Single helper writes the bytes AND marks the node's pixels
        // dirty so the next render queues a thumbnail readback —
        // forgetting the dirty mark was the bug that left
        // `open_document`-loaded layers without thumbnails until the
        // user's first edit.
        self.compositor
            .upload_node_pixels(&self.gpu.queue, id, rgba);

        // Position relative to the active node. `resolve_anchor_target` maps a
        // filter anchor (the active id while editing a mask) to its host, so
        // the pasted layer lands as the host's sibling rather than nested under
        // it — the same anchor resolution the document's `add_*` helpers use.
        let target = self.doc.resolve_anchor_target(active_layer_id);
        self.doc.move_layer(id, target);

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.push_undo(Box::new(LayerAddAction::new(id, parent, pos)));

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

    // -----------------------------------------------------------------------
    // Rich (layer-with-metadata) copy/paste — cross-tab interop
    // -----------------------------------------------------------------------

    /// Like `copy`, but also captures CPU-side layer metadata (blend mode,
    /// opacity, name, mask presence) so the receiving tab can restore it on
    /// paste. Pixel bytes still flow through the existing async readback;
    /// the metadata is stitched in when `complete_copy` runs.
    ///
    /// Returns `None` immediately. The result is available via
    /// `poll_copy_rich_result()` once the readback lands (next frame) — so the
    /// wire verb is fire-and-forget despite the non-`()` return.
    #[handler(post)]
    pub fn copy_layer_rich(&mut self, id: LayerId) -> Option<()> {
        // Snapshot metadata up-front. Layers can mutate while the readback
        // is in flight; pinning at copy time matches user intent and keeps
        // the snapshot independent of whatever happens before completion.
        //
        // Rich metadata (blend mode, opacity, …) only exists for raster
        // layers. The active paint target when editing a mask is the mask
        // *filter* id, which has no layer metadata — fall through to a plain
        // pixel copy (`pending_rich_metadata` left `None` → `complete_copy`
        // builds an image clip). Voids regenerate from params, so there's
        // nothing to read back; cross-tab clipboard for voids would need its
        // own JSON path (out of scope for this change).
        let meta = match self.doc.layer(id) {
            Some(Layer::Raster(layer)) => {
                let mask = layer.filters.iter().find_map(|mid| {
                    let m = self.doc.find_filter(*mid)?;
                    if !m.is_mask() {
                        return None;
                    }
                    let bounds = m.pixels().map(|p| p.bounds).unwrap_or(layer.pixels.bounds);
                    Some(RichCopyMask {
                        name: m.common.name.clone(),
                        visible: m.common.visible,
                        bounds,
                    })
                });
                Some(RichCopyMetadata {
                    name: layer.common.name.clone(),
                    visible: layer.common.visible,
                    locked: layer.common.locked,
                    opacity: layer.blend.opacity,
                    blend_mode: layer.blend.blend_mode.type_id.to_string(),
                    mask,
                })
            }
            // Mask filter: copy pixels only, no rich metadata.
            _ if self.doc.is_filter(id) => None,
            // Groups / voids / unknown ids have no pixel-readback path here.
            _ => return None,
        };
        self.pending_rich_metadata = meta;

        // Reuse the standard copy path for the pixel readback. When
        // `complete_copy` runs and finds `pending_rich_metadata` populated,
        // it builds the `LayerClipboard` JSON and stashes it on
        // `pending_layer_clip`.
        if self.has_selection() && self.selection_cpu_cache().is_none() {
            self.pending_copy = Some(PendingCopy {
                layer_id: id,
                is_cut: false,
            });
            return None;
        }
        self.start_copy_readback(id, false);
        None
    }

    /// Drain the most recent rich-copy result. Returns the JSON-serialised
    /// `LayerClipboard` when the async readback has completed, otherwise
    /// `None`. Mirrors `poll_copy_result`.
    #[handler]
    pub fn poll_copy_rich_result(&mut self) -> Option<String> {
        self.pending_layer_clip.take()
    }

    /// Paste a `LayerClipboard` JSON envelope as a new layer. Restores the
    /// source's name, blend mode, opacity, visibility, and pixel data. If
    /// the source had a mask, an empty (fully opaque) mask is attached at
    /// the recorded bounds — v1 doesn't carry mask pixels yet (see
    /// [`crate::clipboard::LayerClipboard`]).
    ///
    /// Returns the new layer's id, or `None` if the JSON failed to parse
    /// or the blend-mode/pixel data was malformed.
    pub fn paste_layer_rich(
        &mut self,
        json: &str,
        active_layer_id: Option<LayerId>,
    ) -> Option<LayerId> {
        let clip = LayerClipboard::from_json(json).ok()?;
        let pixels = clip.decode_pixels().ok()?;
        let expected_len = (clip.bounds.width * clip.bounds.height * 4) as usize;
        if pixels.len() != expected_len {
            return None;
        }

        let id = self.paste_image(
            clip.bounds.width,
            clip.bounds.height,
            &pixels,
            clip.bounds.x,
            clip.bounds.y,
            active_layer_id,
        );

        // Restore metadata onto the freshly-created raster layer.
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(id) {
            r.common.name = clip.name.clone();
            r.common.visible = clip.visible;
            r.common.locked = clip.locked;
            r.blend.opacity = clip.opacity;
            if let Some(reg) = blend_mode::registry().get(&clip.blend_mode) {
                r.blend.blend_mode = reg;
            }
        }

        // Restore mask presence (without pixels — v1).
        if clip.mask.is_some() {
            self.add_mask(id);
        }

        Some(id)
    }
}
