//! Floating content — paste-in-place and interactive transforms.

use super::{DarklyEngine, PendingTransform};
use crate::document::MoveTarget;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::transform::{Affine2D, ClearShape, FloatingContent, FloatingMode, IDENTITY};
use crate::layer::{Layer, LayerId};
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
    pub fn floating_target_layer(&self) -> Option<LayerId> {
        self.floating.as_ref().map(|fc| fc.target_layer)
    }

    /// Paste from the internal clipboard as floating content on the current
    /// layer/mask. Returns true if floating content was created.
    pub fn paste_in_place_floating(&mut self, layer_id: LayerId) -> bool {
        // Auto-commit any existing floating content first.
        self.auto_commit_floating();

        let clip = match self.clipboard.as_ref().and_then(|c| c.as_image()) {
            Some(c) => c,
            None => return false,
        };

        let source_origin = (clip.offset_x, clip.offset_y);
        let source_width = clip.width;
        let source_height = clip.height;

        // Upload flat RGBA data to GPU for preview. The target node's format
        // is read off `compositor.node_texture(layer_id).format` inside the
        // compositor — the engine never speaks the word "mask" here.
        self.compositor.set_floating_content(
            &self.gpu.device,
            &self.gpu.queue,
            &clip.data,
            source_origin,
            source_width,
            source_height,
            layer_id,
        );

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            mode: FloatingMode::Paste {
                created_layer_id: None,
            },
        });

        // Build the preview now so the paste is visible on the first frame.
        // Without this, `set_floating_content` allocates an empty preview
        // texture and the host's blend pass samples uninitialized pixels
        // until the user drags (which triggers `update_floating_matrix` →
        // `update_floating_preview`). The paste appeared invisible until
        // the first move.
        self.update_floating_preview();

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
        active_layer_id: Option<LayerId>,
    ) -> LayerId {
        // Auto-commit any existing floating content first.
        self.auto_commit_floating();

        // Size the new layer to fit the paste, so off-canvas pixels are
        // preserved when the floating commits.
        let layer_bounds = crate::coord::CanvasRect::from_xywh(offset_x, offset_y, width, height);

        // Create the target layer (no undo entry yet — pushed at commit).
        let new_id = self.doc.add_raster_layer(None);
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(new_id) {
            r.common.name = "Pasted Layer".to_string();
            r.pixels.bounds = layer_bounds;
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
        );

        self.floating = Some(FloatingContent {
            source_origin: (offset_x, offset_y),
            source_width: width,
            source_height: height,
            matrix: IDENTITY,
            target_layer: new_id,
            mode: FloatingMode::Paste {
                created_layer_id: Some(new_id),
            },
        });

        // Build the preview so the paste is visible on the first frame.
        // See `paste_in_place_floating` for the full reasoning.
        self.update_floating_preview();

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
    pub fn begin_transform(&mut self, layer_id: LayerId) -> bool {
        self.auto_commit_floating();

        // Active node may be either a raster layer or a mask modifier — both
        // own a `PixelBuffer` and a node texture. The doc lookup just verifies
        // the id resolves to one of those two; the rest of the flow uses the
        // node id uniformly.
        if self.doc.layer(layer_id).is_none() && self.doc.find_modifier(layer_id).is_none() {
            return false;
        }

        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Determine source bounds.
        if self.has_selection() {
            // Selection bounds come from cpu_cache (populated eagerly on
            // upload or lazily from async readback). If unavailable, defer.
            if self.selection_pixel_bounds().is_none() {
                let recomputed = {
                    let data = self.selection_cpu_cache();
                    data.map(|d| {
                        crate::mask::pixel_bounds_r8(d, self.doc.width, self.doc.height).map(
                            |[x, y, w, h]| {
                                crate::coord::CanvasRect::from_xywh(x as i32, y as i32, w, h)
                            },
                        )
                    })
                };
                match recomputed {
                    Some(bounds) => self.set_selection_pixel_bounds(bounds),
                    None => {
                        // Cache not ready — defer until SelectionReadback completes.
                        self.pending_transform = Some(PendingTransform { node_id: layer_id });
                        return false;
                    }
                }
            }
            let bounds = match self.selection_pixel_bounds() {
                Some(b) => b,
                None => return false,
            };

            let x = bounds.x0().max(0);
            let y = bounds.y0().max(0);
            let w = bounds.width.min(canvas_w.saturating_sub(x as u32));
            let h = bounds.height.min(canvas_h.saturating_sub(y as u32));

            if w == 0 || h == 0 {
                return false;
            }

            self.setup_transform(layer_id, (x, y), w, h);
            true
        } else {
            // No selection — use compositor content bounds.
            if let Some(bounds) = self.compositor.content_bounds(layer_id) {
                // content_bounds are in layer-local coords; translate to
                // canvas-space via the layer texture so callers (and floating
                // preview/uniforms) see canvas coords.
                let [bx, by, bw, bh] = bounds;
                if bw == 0 || bh == 0 {
                    return false;
                }
                let canvas_origin = self
                    .compositor
                    .node_texture(layer_id)
                    .map(|t| t.layer_to_canvas(crate::coord::LayerPoint::new(bx, by)))
                    .unwrap_or(crate::coord::CanvasPoint::new(bx as i32, by as i32));
                self.setup_transform(layer_id, (canvas_origin.x, canvas_origin.y), bw, bh);
                true
            } else {
                // Bounds not yet computed — request async GPU compute.
                self.compositor
                    .request_content_bounds(&self.gpu.device, &self.gpu.queue, layer_id);
                self.pending_transform = Some(super::PendingTransform { node_id: layer_id });
                false
            }
        }
    }

    /// Common setup logic for interactive transforms — copies source region
    /// to the floating texture and stores a `ClearShape` so commit (and the
    /// per-frame preview) can erase the source rect on the right surface
    /// before re-rendering. The live target texture is **not** mutated:
    /// preview is a derived view, and commit applies the clear to the live
    /// target right before writing the transform.
    pub(crate) fn setup_transform(
        &mut self,
        node_id: LayerId,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
    ) {
        let layer_id = node_id;
        let format = self
            .compositor
            .node_texture(node_id)
            .map(|t| t.format)
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;

        // Layer's current canvas-space extent (paste-extent layers may run
        // off-canvas), and the canvas-space rect of the source region
        // clipped against it.
        let layer_extent = self
            .compositor
            .node_texture(node_id)
            .map(|t| t.canvas_extent())
            .unwrap_or(crate::coord::CanvasRect::from_xywh(
                0, 0, canvas_w, canvas_h,
            ));
        let canvas_source = crate::coord::CanvasRect::from_xywh(
            source_origin.0,
            source_origin.1,
            source_width,
            source_height,
        );
        let canvas_save_rect = layer_extent
            .intersect(canvas_source)
            .unwrap_or_else(|| crate::coord::CanvasRect::from_xywh(0, 0, 0, 0));

        // Bail if the target node has no GPU texture (caller already
        // validated the id, but a freshly-added paste-extent layer can be
        // pre-bounds-allocation).
        if self.compositor.node_texture(node_id).is_none() {
            return;
        }

        // Copy source region from the live target into the floating source
        // texture (premultiplied for RGBA, raw for R8).
        self.gpu.encode("transform-copy-source", |encoder| {
            self.compositor.set_floating_content_from_gpu(
                &self.gpu.device,
                &self.gpu.queue,
                encoder,
                source_origin,
                source_width,
                source_height,
                layer_id,
            );
        });

        // If selection is active, mask the source texture so only selected
        // pixels are included in the transform. Snapshot the selection into
        // a dedicated R8 so commit can replay it after the live selection
        // clear at the end of setup zeroes the marching ants.
        let has_selection = self.has_selection();
        let clear_shape = if has_selection {
            let cropped_sel_bg =
                self.upload_cropped_selection_r8(source_origin, source_width, source_height);

            if let Some(sel_bg) = &cropped_sel_bg {
                if let Some(source_tex) = self.compositor.transform_source_texture() {
                    let target = GpuPaintTarget {
                        texture: source_tex.0,
                        view: source_tex.1,
                        format,
                        width: source_width,
                        height: source_height,
                        offset_x: 0,
                        offset_y: 0,
                        canvas_width: source_width,
                        canvas_height: source_height,
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

            ClearShape::Selection {
                mask_bind_group: self.snapshot_selection_for_clear(),
            }
        } else {
            ClearShape::Rect(canvas_save_rect)
        };

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            mode: FloatingMode::Transform { clear_shape },
        });

        // Selection was used to define what gets picked up — clear it now
        // so marching ants disappear and the transform output isn't clipped
        // by the live selection during commit.
        if has_selection {
            let bounds = self.selection_pixel_bounds();
            if let Some(state) = self.compositor.selection_state_mut() {
                state.clear_region(&self.gpu.queue, bounds);
            }
            self.set_selection_pixel_bounds(None);
            self.set_selection_active(false);
            self.invalidate_selection_cpu_cache();

            self.selection_overlay.clear();
            self.push_merged_overlay();
        }

        // Render the initial preview so the host's blend reads the right
        // texture from the very first frame after setup.
        self.update_floating_preview();
    }

    /// Build (or rebuild) the floating preview texture for the current
    /// matrix and clear shape. Called after `setup_transform` and on every
    /// `update_floating_matrix`.
    fn update_floating_preview(&mut self) {
        let Some(fc) = self.floating.as_ref() else {
            return;
        };
        let clear_shape = match &fc.mode {
            FloatingMode::Transform { clear_shape } => Some(clear_shape),
            FloatingMode::Paste { .. } => None,
        };
        self.compositor.update_floating_preview(
            &self.gpu.device,
            &self.gpu.queue,
            &self.paint_pipelines,
            &fc.matrix,
            fc.source_origin,
            fc.source_width,
            fc.source_height,
            clear_shape,
        );
    }

    /// Snapshot the live GPU selection into a fresh canvas-sized R8 texture
    /// and return a paint-pipeline bind group sampling it. The returned
    /// bind group keeps the underlying texture alive for its lifetime, so
    /// it remains valid after the selection clear zeroes the live selection
    /// at the end of `setup_transform`.
    fn snapshot_selection_for_clear(&self) -> wgpu::BindGroup {
        let canvas_w = self.doc.width;
        let canvas_h = self.doc.height;
        let snap_tex = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transform-clear-sel-snap"),
            size: wgpu::Extent3d {
                width: canvas_w,
                height: canvas_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let live_tex = self
            .compositor
            .selection_state()
            .expect("snapshot_selection_for_clear: selection_state allocated")
            .texture();
        self.gpu.encode("transform-clear-sel-snap", |encoder| {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: live_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &snap_tex,
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
        let view = snap_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("transform-clear-sel-snap-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        self.paint_pipelines
            .create_selection_bind_group(&self.gpu.device, &view, &sampler)
    }

    /// Update the floating content's transform matrix and rebuild the
    /// per-frame preview texture so the host's blend reads the new shape.
    pub fn update_floating_matrix(&mut self, matrix: Affine2D) {
        if let Some(fc) = self.floating.as_mut() {
            fc.matrix = matrix;
        } else {
            return;
        }
        self.update_floating_preview();
        self.compositor.mark_dirty();
    }

    /// Commit floating content: render transformed pixels into the target
    /// layer/mask texture via a GPU render pass.
    pub fn commit_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        let layer_id = fc.target_layer;
        // Format comes from the unified node-texture pool. Both raster layer
        // (RGBA8) and mask modifier (R8) targets resolve through the same call.
        let format = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.format)
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);

        // Compute tight affected rect = union(source bounds, transformed
        // bounds), in CANVAS coordinates. Intentionally NOT clamped to
        // canvas — layer textures may extend past the canvas, and content
        // dragged past the canvas edge must survive on the layer so it
        // reappears when moved back. We grow the target below to fit.
        let (min_x, min_y, max_x, max_y) = fc.transformed_bounds();
        let (sox, soy) = fc.source_origin;
        let src_max_x = sox + fc.source_width as i32;
        let src_max_y = soy + fc.source_height as i32;
        let affected_canvas = crate::coord::CanvasRect::from_xywh(
            min_x.min(sox),
            min_y.min(soy),
            (max_x.max(src_max_x) - min_x.min(sox)).max(0) as u32,
            (max_y.max(src_max_y) - min_y.min(soy)).max(0) as u32,
        );

        // Grow the target (or its host, for mask modifiers) so the layer
        // texture can hold any portion of the affected rect that lies
        // outside its current bounds — including pixels past the canvas
        // edge. Best-effort: if growth is refused (cap, or target is
        // neither raster nor modifier with a raster host), commit falls
        // back to the pre-grow extent and the texture-side clip below
        // still keeps the commit consistent.
        let _ = self.grow_node_to_fit(layer_id, affected_canvas);

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
            self.undo_stack.push(
                &mut self.doc,
                Box::new(LayerAddAction::new(layer_id, parent, pos)),
            );

            self.compositor.mark_node_pixels_dirty(layer_id);
            self.compositor.clear_floating_content();
            return;
        }

        // Translate the canvas-space affected rect into the target's
        // layer-local frame. After grow, the post-grow extent contains
        // `affected_canvas`; the intersect is just a safety net for the
        // growth-refused path.
        let target_canvas_extent = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.canvas_extent());
        let affected_canvas_rect = match target_canvas_extent {
            Some(extent) => match affected_canvas.intersect(extent) {
                Some(c) => c,
                None => {
                    self.compositor.clear_floating_content();
                    return;
                }
            },
            None => {
                self.compositor.clear_floating_content();
                return;
            }
        };

        // The live target was never destructively touched during the
        // floating session — `setup_transform` only copied source pixels
        // out, and the per-frame preview ran into a dedicated preview
        // texture. So `save_region` here captures the genuine pre-
        // transform state for undo, no un-clear dance required. The
        // target's texture existence was already verified above when
        // `target_canvas_extent` was read.
        macro_rules! layer_frame {
            () => {
                self.compositor
                    .node_texture(layer_id)
                    .unwrap()
                    .canvas_frame()
            };
        }
        let commit_snap = self.gpu.encode_ret("transform-commit-save", |encoder| {
            let frame = layer_frame!();
            self.region_store.save_region(
                &self.gpu.device,
                encoder,
                &frame,
                format,
                affected_canvas_rect,
            )
        });

        // Apply ClearShape to the live target, then run commit. Same
        // sequence the preview applies on every drag, but here it lands
        // on the live texture and survives the floating session.
        self.gpu.encode("transform-commit", |encoder| {
            let frame = layer_frame!();
            let entry = self.region_store.commit_region(
                encoder,
                layer_id,
                &frame,
                &commit_snap,
                affected_canvas_rect,
            );

            if let FloatingMode::Transform {
                ref clear_shape, ..
            } = fc.mode
            {
                let target = self
                    .compositor
                    .node_texture(layer_id)
                    .map(|t| GpuPaintTarget::from_node(t, self.doc.width, self.doc.height));
                if let Some(target) = target {
                    match clear_shape {
                        ClearShape::Rect(rect) => {
                            let canvas_rect =
                                [rect.x0(), rect.y0(), rect.width as i32, rect.height as i32];
                            target.clear_rect(
                                encoder,
                                &self.paint_pipelines,
                                &self.gpu.queue,
                                canvas_rect,
                            );
                        }
                        ClearShape::Selection { mask_bind_group } => {
                            target.erase_with_selection(
                                encoder,
                                &self.paint_pipelines,
                                &self.gpu.queue,
                                mask_bind_group,
                            );
                        }
                    }
                }
            }

            self.compositor.commit_floating_to_texture(
                &self.gpu.device,
                encoder,
                &self.gpu.queue,
                &fc.matrix,
                fc.source_origin,
                fc.source_width,
                fc.source_height,
            );

            self.undo_stack
                .push(&mut self.doc, Box::new(GpuRegionAction::new(entry)));
        });

        // Clean up GPU state
        self.compositor.mark_node_pixels_dirty(layer_id);
        self.compositor.clear_floating_content();
    }

    /// Cancel floating content: drop the floating session. The live target
    /// texture was never mutated during a transform (preview lives on a
    /// separate texture), so cancel is a pure session-state reset.
    pub fn cancel_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        if let FloatingMode::Paste {
            created_layer_id: Some(id),
        } = fc.mode
        {
            // Paste auto-created a target layer; drop it silently. No undo
            // entry to maintain — `LayerAddAction` is only pushed on commit.
            self.doc.detach_for_undo(id);
            self.compositor.dispose_layer(id);
            self.compositor.mark_dirty();
        }
        // FloatingMode::Transform and Paste-onto-existing both leave the
        // target texture exactly as it was at setup_transform — nothing to
        // restore.

        self.compositor.clear_floating_content();
        self.compositor.mark_dirty();
    }
}
