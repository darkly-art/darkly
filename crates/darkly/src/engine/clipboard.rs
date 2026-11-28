//! Copy, cut, paste operations.

use darkly_macros::handlers;

use super::rendering::commit_undo_region;
use super::types::ClipboardExport;
use super::{DarklyEngine, PendingCopy, ReadbackContext, RichCopyMask, RichCopyMetadata};
use crate::clipboard::{
    Clipboard, ClipboardRect, ImageClip, LayerClipboard, MaskClipboard,
    LAYER_CLIPBOARD_SCHEMA_VERSION,
};
use crate::document::PixelTransformBoundsPolicy;
use crate::gpu::blend_mode;
use crate::gpu::layer_readback::{self, LayerReadbackExtent};
use crate::gpu::paint_target::GpuPaintTarget;
use crate::layer::{Layer, LayerId};
use crate::undo::GpuRegionAction;

#[handlers]
impl DarklyEngine {
    /// Copy the active layer's content (masked by selection) into the internal
    /// clipboard. Kicks off an async GPU readback; the result is available via
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
            // Selection cache not ready: defer until SelectionReadback completes.
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

    /// Start the readback for copy (or cut).
    ///
    /// The clip is the node's content over the copied region (the selection's
    /// bounds, or the window), built on the CPU by
    /// [`LayerReadbackExtent::clip_rgba`] once the readback lands, with the
    /// selection scaling its coverage. A node whose transform extraction spans
    /// its whole document extent (a mask, whose white past its texture is
    /// content) carries the whole region; any other node carries only what its
    /// texture holds.
    ///
    /// A cut erases the node in the same submit, after the read: through the
    /// selection (`node.a *= 1 - sel`) with a whole-node undo snapshot, or the
    /// whole node without one.
    pub(crate) fn start_copy_readback(&mut self, layer_id: LayerId, is_cut: bool) {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let canvas_origin = self.doc.canvas_origin;

        let Some((node_extent, format)) = self
            .compositor
            .node_texture(layer_id)
            .map(|t| (t.canvas_extent(), t.format()))
        else {
            return;
        };

        // Compute copy region from selection bounds (or full canvas).
        let region = match self.copy_region_from_selection(canvas_w, canvas_h) {
            Some(r) => r,
            None => {
                // Selection bounds unknown: defer.
                self.pending_copy = Some(PendingCopy { layer_id, is_cut });
                return;
            }
        };
        let [rx, ry, rw, rh] = region;
        if rw == 0 || rh == 0 {
            return;
        }
        let region_plane = crate::coord::WindowRect::from_xywh(rx as i32, ry as i32, rw, rh)
            .to_canvas(canvas_origin);
        let carries_uncovered = self
            .doc
            .pixel_transform_semantics(layer_id)
            .is_some_and(|s| s.bounds_policy == PixelTransformBoundsPolicy::DocumentExtent);
        let clip = if carries_uncovered {
            region_plane
        } else {
            match node_extent.intersect(region_plane) {
                Some(r) => r,
                None => return,
            }
        };
        let clip_window = clip.to_window(canvas_origin);
        // `copy` and `cut` wait for the selection's CPU cache, so with a
        // selection active this is always `Some`.
        let selection = self.cropped_selection_pixels(
            (clip_window.x0(), clip_window.y0()),
            clip.width,
            clip.height,
        );
        let erase_selection = is_cut && selection.is_some();

        // Save the whole node for the selection cut's undo before any modification.
        let cut_snapshot = if erase_selection {
            self.compositor
                .node_texture(layer_id)
                .map(|t| t.canvas_frame())
                .and_then(|frame| {
                    self.gpu.encode_ret("cut-save", |encoder| {
                        self.region_scratch.save_region(
                            &self.gpu.device,
                            encoder,
                            &frame,
                            format,
                            frame.canvas_extent,
                        )
                    })
                })
        } else {
            None
        };

        let target = self
            .compositor
            .node_texture(layer_id)
            .map(|t| GpuPaintTarget::from_node(t, self.doc.canvas_rect()))
            .expect("node texture checked above");
        let mut encoder = crate::gpu::paint_target::PaintCommandEncoder::new(
            &self.gpu.device,
            &self.gpu.queue,
            &self.paint_pipelines,
            "copy",
            1,
        );
        let (request, extent) = encoder.with_raw(|raw| {
            layer_readback::request_layer_region_readback(&self.gpu.device, raw, &target, clip)
        });
        if erase_selection {
            target.multiply_alpha_by_inverse_mask(
                &mut encoder,
                &self.paint_pipelines,
                &self.gpu.queue,
                self.compositor
                    .selection_state()
                    .expect("has_selection true → selection_state allocated")
                    .selection_bind_group(),
            );
        }
        if let Some(request) = request {
            self.readbacks.submit(
                request,
                ReadbackContext::Copy {
                    region: clip,
                    extent,
                    selection,
                },
            );
            encoder.submit();
        } else {
            // The texture does not reach the clip: it is all uncovered value.
            encoder.submit();
            self.complete_copy(clip, extent, selection, Vec::new());
        }

        if let Some(snap) = cut_snapshot {
            let entry = commit_undo_region(
                &self.gpu,
                &self.region_scratch,
                &mut self.readbacks,
                "cut-commit",
                layer_id,
                &snap,
            );
            self.push_undo(Box::new(GpuRegionAction::new(entry)));
            self.compositor.mark_node_pixels_dirty(layer_id);
        } else if is_cut && !erase_selection {
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

    /// Complete a pending copy once its readback is available: project the
    /// node's texels into the clip over the plane rect `region`, which is also
    /// the clip's plane position (paste in place lands it there).
    pub(crate) fn complete_copy(
        &mut self,
        region: crate::coord::CanvasRect,
        extent: LayerReadbackExtent,
        selection: Option<Vec<u8>>,
        pixels: Vec<u8>,
    ) {
        let rgba = extent.clip_rgba(&pixels, region, selection.as_deref());
        let clip =
            ImageClip::from_rgba(region.width, region.height, rgba, region.x0(), region.y0());
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
    }

    /// Cut = copy + clear. The clear happens on GPU during start_copy_readback.
    /// Returns `None` immediately; result available via `poll_copy_result()`.
    #[handler]
    pub fn cut(&mut self, id: LayerId) -> Option<ClipboardExport> {
        // Accept a filter id (mask edit target) as well as a layer; see
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
        // dirty so the next render queues a thumbnail readback;
        // forgetting the dirty mark was the bug that left
        // `open_document`-loaded layers without thumbnails until the
        // artist's first edit.
        self.compositor
            .upload_node_pixels(&self.gpu.queue, id, rgba);

        // Position relative to the active node. `resolve_anchor_target` maps a
        // filter anchor (the active id while editing a mask) to its host, so
        // the pasted layer lands as the host's sibling rather than nested under
        // it: the same anchor resolution the document's `add_*` helpers use.
        // A paste is an add, not a move: `place_layer` takes the boundary
        // policy, so an anchor above the viewport divider cannot pull a raster
        // into the run.
        let target = self.doc.resolve_anchor_target(active_layer_id);
        self.doc.place_layer(id, target);

        self.record_entity_add(id);

        id
    }

    /// Paste the internal clipboard INTO the active target (layer or mask) at
    /// the source's original position, committed immediately with no transform
    /// session. This is the committed counterpart of `paste_in_place_floating`
    /// (used when "activate transform after paste" is off); both float the clip
    /// onto the active node and share the single `commit_floating` path, which
    /// writes RGBA layers and R8 masks alike. Returns the target id, or `None`
    /// when there's no active target or nothing to paste.
    pub fn paste_in_place(&mut self, active_layer_id: Option<LayerId>) -> Option<LayerId> {
        let id = active_layer_id?;
        if !self.paste_in_place_floating(id) {
            return None;
        }
        self.commit_floating();
        Some(id)
    }

    // -----------------------------------------------------------------------
    // Rich (layer-with-metadata) copy/paste: cross-tab interop
    // -----------------------------------------------------------------------

    /// Like `copy`, but also captures CPU-side layer metadata (blend mode,
    /// opacity, name, mask presence) so the receiving tab can restore it on
    /// paste. Pixel bytes still flow through the existing async readback;
    /// the metadata is stitched in when `complete_copy` runs.
    ///
    /// Returns `None` immediately. The result is available via
    /// `poll_copy_rich_result()` once the readback lands (next frame), so the
    /// wire verb is fire-and-forget despite the non-`()` return.
    #[handler(post)]
    pub fn copy_layer_rich(&mut self, id: LayerId) -> Option<()> {
        // Snapshot metadata up-front. Layers can mutate while the readback
        // is in flight; pinning at copy time matches artist intent and keeps
        // the snapshot independent of whatever happens before completion.
        //
        // Rich metadata (blend mode, opacity, …) only exists for raster
        // layers. The active paint target when editing a mask is the mask
        // *filter* id, which has no layer metadata, so fall through to a plain
        // pixel copy (`pending_rich_metadata` left `None` → `complete_copy`
        // builds an image clip). Voids regenerate from params, so there's
        // nothing to read back; cross-tab clipboard for voids would need its
        // own JSON path (out of scope for this change).
        //
        // A region copy (selection active) captures flat composited pixels and
        // must NOT carry the mask: a mask belongs only to a whole-layer copy
        // (which is a layer duplication in spirit). Gate the mask capture on
        // provenance.
        let is_region_copy = self.has_selection();
        let meta = match self.doc.layer(id) {
            Some(Layer::Raster(layer)) => {
                let mask = if is_region_copy {
                    None
                } else {
                    layer.filters.iter().find_map(|mid| {
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
                    })
                };
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
    /// the recorded bounds (v1 doesn't carry mask pixels yet, see
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

        // Restore mask presence (without pixels, v1). Use the unseeded attach:
        // the pasted layer's `EntityAddAction` already covers the mask in its
        // subtree, and seeding from the receiving document's active selection
        // would wrongly turn that selection into the mask.
        if clip.mask.is_some() {
            self.add_mask_unseeded(id);
            self.compositor.mark_dirty();
        }

        Some(id)
    }
}
