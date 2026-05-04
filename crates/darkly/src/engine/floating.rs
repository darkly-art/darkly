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
        let new_id = self.doc.add_raster_layer();
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

    /// Common setup logic for interactive transforms — saves pre-clear state,
    /// copies source region to floating texture, clears source on layer.
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

        // Look up the target node's bounds so we can translate the
        // canvas-space `source_origin` into layer-local coords for any
        // operation that touches the texture directly.
        let layer_extent = self
            .compositor
            .node_texture(node_id)
            .map(|t| t.canvas_extent())
            .unwrap_or(crate::coord::CanvasRect::from_xywh(
                0, 0, canvas_w, canvas_h,
            ));

        // Canvas-space rect of the source region, clipped to the layer's
        // current extent. This is the slice of the layer texture that the
        // transform will modify, and the slice we need to snapshot so
        // cancel/undo can restore it.
        let canvas_source = crate::coord::CanvasRect::from_xywh(
            source_origin.0,
            source_origin.1,
            source_width,
            source_height,
        );
        let canvas_save_rect = layer_extent
            .intersect(canvas_source)
            .unwrap_or_else(|| crate::coord::CanvasRect::from_xywh(0, 0, 0, 0));

        // Save the source region to scratch (pre-clear snapshot for undo
        // and cancel). Must happen before the clear.
        let layer_frame = self
            .compositor
            .node_texture(node_id)
            .map(|t| t.canvas_frame());
        let cancel_snapshot = match layer_frame {
            Some(frame) => {
                // Source rect may exceed canvas bounds (paste-extent layer
                // transform). Pre-grow the scratch.
                self.region_store.ensure_scratch_capacity(
                    &self.gpu.device,
                    layer_extent.width,
                    layer_extent.height,
                );
                Some(self.gpu.encode_ret("transform-save", |encoder| {
                    self.region_store
                        .save_region(encoder, &frame, format, canvas_save_rect)
                }))
            }
            None => None,
        };
        let cancel_snapshot = match cancel_snapshot {
            Some(s) => s,
            None => return,
        };

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
            );
        });

        // If selection is active, mask the source texture so only selected pixels
        // are included in the transform. Also clear only selected pixels on the layer.
        let has_selection = self.has_selection();
        let clear_shape = if has_selection {
            // Upload a cropped selection mask matching the source region dimensions.
            let cropped_sel_bg =
                self.upload_cropped_selection_r8(source_origin, source_width, source_height);

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

            // Snapshot the live selection texture into a dedicated R8 so
            // commit can replay the exact selection shape for the layer
            // re-clear, even after the selection clear (below) zeroes the
            // live selection at the end of setup. The snapshot owns its
            // texture for the lifetime of the floating session.
            let snap_bg = self.snapshot_selection_for_clear();

            // Clear selected pixels on the layer using erase_with_selection
            // — same bind group we just snapshotted from, applied to the
            // node target. Format dispatch is internal to GpuPaintTarget.
            let layer_target = self
                .compositor
                .node_texture(layer_id)
                .map(|t| GpuPaintTarget::from_node(t, canvas_w, canvas_h));
            if let Some(target) = layer_target {
                self.gpu.encode("transform-clear-sel", |encoder| {
                    target.erase_with_selection(
                        encoder,
                        &self.paint_pipelines,
                        &self.gpu.queue,
                        &snap_bg,
                    );
                });
            }

            ClearShape::Selection {
                mask_bind_group: snap_bg,
            }
        } else {
            // No selection — clear the layer-local source region on the layer.
            let target = self
                .compositor
                .node_texture(layer_id)
                .map(|t| GpuPaintTarget::from_node(t, canvas_w, canvas_h));
            if let Some(target) = target {
                // clear_rect is canvas-space; the saved canvas rect is
                // already canvas-aligned.
                let canvas_rect = [
                    canvas_save_rect.x0(),
                    canvas_save_rect.y0(),
                    canvas_save_rect.width as i32,
                    canvas_save_rect.height as i32,
                ];
                self.gpu.encode("transform-clear", |encoder| {
                    target.clear_rect(encoder, &self.paint_pipelines, &self.gpu.queue, canvas_rect);
                });
            }
            ClearShape::Rect(canvas_save_rect)
        };

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            // `cancel_snapshot` carries the pre-clear pixels at the
            // source rect (used by `cancel_floating` via
            // `restore_from_scratch`). `clear_shape` describes the shape
            // of the layer clear setup_transform just applied — replayed
            // by `commit_floating` after its un-clear/save sequence so
            // the transform render doesn't leave duplicate source pixels
            // at the original position.
            mode: FloatingMode::Transform {
                cancel_snapshot,
                clear_shape,
            },
        });

        // Selection was used to define what gets picked up — clear it now so
        // the marching ants disappear and the transform output isn't clipped.
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
        // Format comes from the unified node-texture pool. Both raster layer
        // (RGBA8) and mask modifier (R8) targets resolve through the same call.
        let format = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.format)
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);

        // Compute tight affected rect = union(source bounds, transformed bounds),
        // clamped to canvas. This is in CANVAS coordinates.
        let canvas_w = self.doc.width as i32;
        let canvas_h = self.doc.height as i32;
        let (min_x, min_y, max_x, max_y) = fc.transformed_bounds();
        let (sox, soy) = fc.source_origin;
        let src_max_x = sox + fc.source_width as i32;
        let src_max_y = soy + fc.source_height as i32;
        let canvas_clip =
            crate::coord::CanvasRect::from_xywh(0, 0, canvas_w as u32, canvas_h as u32);
        let affected_canvas = crate::coord::CanvasRect::from_xywh(
            min_x.min(sox),
            min_y.min(soy),
            (max_x.max(src_max_x) - min_x.min(sox)).max(0) as u32,
            (max_y.max(src_max_y) - min_y.min(soy)).max(0) as u32,
        )
        .intersect(canvas_clip);

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

            self.compositor.mark_node_pixels_dirty(layer_id);
            self.compositor.clear_floating_content();
            return;
        }

        // Translate the canvas-space affected rect into the target's
        // layer-local frame. This is the boundary that *would have* been
        // silently wrong before — the typed API forces the conversion.
        let target_canvas_extent = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.canvas_extent());
        let affected_canvas_rect = match (affected_canvas, target_canvas_extent) {
            (Some(rect), Some(extent)) => match rect.intersect(extent) {
                Some(c) => c,
                None => {
                    self.compositor.clear_floating_content();
                    return;
                }
            },
            _ => {
                self.compositor.clear_floating_content();
                return;
            }
        };

        // Save the pre-transform layer state at the affected rect.
        //
        // Transform mode's `setup_transform` already cleared the source
        // pixels on the layer — so reading the layer right now would
        // capture the post-clear state, and undoing later would leave
        // those pixels transparent instead of restoring originals. Fix:
        // un-clear via `cancel_snapshot` first, then save. This composes
        // cleanly because:
        //   - the cancel snapshot is already in scratch
        //   - after the un-clear, the live layer matches its pre-clear
        //     state — exactly what undo wants
        //   - the path-B save then overwrites scratch with affected_rect,
        //     invalidating the cancel snapshot, which is fine because
        //     `commit_floating` consumes the FloatingContent by takes-self
        //     pattern (cancel can no longer run on this content).
        //
        // Paste mode never clears the layer, so the un-clear is skipped.
        // Pre-resolve the canvas extent (Copy) so we don't carry a borrow
        // of self.compositor across the closures below.
        let layer_canvas_extent = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.canvas_extent());
        let layer_canvas_extent = match layer_canvas_extent {
            Some(e) => e,
            None => {
                self.compositor.clear_floating_content();
                return;
            }
        };
        // Helper to materialise a CanvasFrame inside a closure without
        // extending an outer borrow.
        macro_rules! layer_frame {
            () => {
                self.compositor
                    .node_texture(layer_id)
                    .unwrap()
                    .canvas_frame()
            };
        }
        if let FloatingMode::Transform {
            ref cancel_snapshot,
            ..
        } = fc.mode
        {
            self.gpu.encode("transform-uncleared", |encoder| {
                let frame = layer_frame!();
                self.region_store.restore_from_scratch(
                    encoder,
                    cancel_snapshot,
                    &frame,
                    cancel_snapshot.saved,
                );
            });
        }
        self.region_store.ensure_scratch_capacity(
            &self.gpu.device,
            layer_canvas_extent.width,
            layer_canvas_extent.height,
        );
        let commit_snap = self.gpu.encode_ret("transform-commit-save", |encoder| {
            let frame = layer_frame!();
            self.region_store
                .save_region(encoder, &frame, format, affected_canvas_rect)
        });

        // Commit the pre-operation state to the undo ring buffer, then
        // render the transform.
        self.gpu.encode("transform-commit", |encoder| {
            let frame = layer_frame!();
            let entry = self.region_store.commit_region(
                encoder,
                layer_id,
                &frame,
                &commit_snap,
                affected_canvas_rect,
            );

            // The un-clear above restored source pixels to the layer at
            // the source rect (so the undo-buffer save captured the
            // pre-transform state). Replay the same clear shape that
            // `setup_transform` applied, before the transform render —
            // otherwise the transform shader's
            // `discard`-outside-transformed-bounds would leave a
            // duplicate copy of the source at its original position.
            // The shape is stored as data so selection and no-selection
            // branches share this single replay path.
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
        self.compositor.mark_node_pixels_dirty(layer_id);
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
                // runs on commit), so just detach the node from the doc and
                // dispose its freshly-allocated GPU resources. No undo
                // entry: the LayerAddAction is only pushed on commit, so
                // there's no future undo path that would need this state.
                if let Some(id) = created_layer_id {
                    self.doc.detach_for_undo(id);
                    self.compositor.dispose_layer(id);
                    self.compositor.mark_dirty();
                }
                // Otherwise: target layer was never modified — no-op.
            }
            FloatingMode::Transform {
                cancel_snapshot, ..
            } => {
                // Restore the pre-clear state from the RegionStore scratch
                // texture (saved during begin_transform).
                //
                // NB: commit_floating's path-B re-save would overwrite this
                // scratch region — but commit_floating consumes the
                // FloatingContent by takes-self pattern, so cancel and
                // commit are mutually exclusive on a given FloatingContent.
                // The cancel path always sees the original setup_transform
                // snapshot intact.
                let layer_frame = self
                    .compositor
                    .node_texture(fc.target_layer)
                    .map(|t| t.canvas_frame());
                if let Some(layer_frame) = layer_frame {
                    self.gpu.encode("cancel-restore", |encoder| {
                        self.region_store.restore_from_scratch(
                            encoder,
                            &cancel_snapshot,
                            &layer_frame,
                            cancel_snapshot.saved,
                        );
                    });
                    self.compositor.mark_node_pixels_dirty(fc.target_layer);
                }
            }
        }

        self.compositor.clear_floating_content();
    }
}
