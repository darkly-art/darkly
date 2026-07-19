//! Floating content — paste-in-place and interactive transforms.

use darkly_macros::handlers;

use super::rendering::commit_undo_region;
use super::{DarklyEngine, PendingTransform};
use crate::coord::CanvasRect;
use crate::document::TransformRelationshipSnapshot;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::transform::{ClearShape, FloatingContent, FloatingMode, Transform};

pub(crate) struct TransformTarget {
    pub node_id: LayerId,
    pub document_bounds: CanvasRect,
    pub extraction_bounds: CanvasRect,
    pub clear_shape: ClearShape,
}

pub(crate) struct TransformSession {
    pub initiator_id: LayerId,
    pub operation_frame: CanvasRect,
    pub operation: Transform,
    pub relationship: TransformRelationshipSnapshot,
    pub targets: Vec<TransformTarget>,
    pub setup_generation: u64,
    pub preview_revision: u64,
    pub has_selection: bool,
}
use crate::layer::{Layer, LayerId};
use crate::undo::{
    CompoundAction, GpuRegionAction, LayerAddAction, PixelBoundsAction, SelectionAction,
    SelectionMetadataAction, UndoAction,
};

#[handlers]
impl DarklyEngine {
    /// Auto-commit any active floating content before performing other edits.
    /// Call this before operations that would conflict with floating content
    /// (layer switch, paint, undo, etc.).
    pub fn auto_commit_floating(&mut self) {
        let _ = self.resolve_transform_conflict();
    }

    /// Generic chokepoint for document edits that can invalidate transform
    /// topology, capability, selection, or target resources.
    pub(crate) fn resolve_transform_conflict(&mut self) -> bool {
        if self.pending_transform.take().is_some() {
            self.transform_setup_generation = self.transform_setup_generation.wrapping_add(1);
        }
        if self.transform_session.is_some() {
            return self.commit_transform_session() && self.transform_session.is_none();
        }
        if self.floating.is_some() {
            self.commit_floating();
            return self.floating.is_none();
        }
        true
    }

    /// Check if there is active floating content.
    #[handler]
    pub fn has_floating(&self) -> bool {
        self.transform_session.is_some() || self.floating.is_some()
    }

    /// Return floating content info for the frontend overlay:
    /// (source_origin_x, source_origin_y, source_width, source_height,
    /// transform). The transform carries its own mode tag, so the gizmo's
    /// current mode is **derived from the document** (the stored `Transform`),
    /// not session-local — a re-`adopt()` can't desync it.
    /// Returns None if no floating content is active.
    pub fn floating_info(&self) -> Option<(f32, f32, f32, f32, Transform)> {
        if let Some(session) = self.transform_session.as_ref() {
            let frame = session.operation_frame;
            Some((
                frame.x0() as f32,
                frame.y0() as f32,
                frame.width as f32,
                frame.height as f32,
                session.operation,
            ))
        } else {
            self.floating.as_ref().map(|fc| {
                (
                    fc.source_origin.0 as f32,
                    fc.source_origin.1 as f32,
                    fc.source_width as f32,
                    fc.source_height as f32,
                    fc.transform,
                )
            })
        }
    }

    /// Return the layer the active floating content will commit to.
    /// Used by the frontend to distinguish "user switched away from the
    /// floating's layer" (dismiss) from "user activated the floating's
    /// own target layer" (keep — paste-as-floating sets active to its
    /// auto-created target).
    #[handler]
    pub fn floating_target_layer(&self) -> Option<LayerId> {
        self.transform_session
            .as_ref()
            .map(|session| session.initiator_id)
            .or_else(|| self.floating.as_ref().map(|fc| fc.target_layer))
    }

    /// Paste from the internal clipboard as floating content on the current
    /// layer/mask. Returns true if floating content was created.
    #[handler]
    pub fn paste_in_place_floating(&mut self, id: LayerId) -> bool {
        if !self.doc.is_node_editable(id) {
            return false;
        }
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
        // is read off `compositor.node_texture(id).format` inside the
        // compositor — the engine never speaks the word "mask" here.
        self.compositor.set_floating_content(
            &self.gpu.device,
            &self.gpu.queue,
            &clip.data,
            source_origin,
            source_width,
            source_height,
            id,
        );

        self.floating = Some(FloatingContent {
            source_origin,
            source_width,
            source_height,
            transform: Transform::identity(),
            target_layer: id,
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

        // Position relative to the active node. `resolve_anchor_target` maps a
        // filter anchor (the active id while editing a mask) to its host, so
        // the pasted layer lands as the host's sibling rather than nested under
        // it — the same anchor resolution the document's `add_*` helpers use.
        let target = self.doc.resolve_anchor_target(active_layer_id);
        self.doc.move_layer(new_id, target);

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
            transform: Transform::identity(),
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

    /// Begin one fixed-membership destructive transform session.
    #[handler]
    pub fn begin_transform(&mut self, id: LayerId) -> bool {
        self.auto_commit_floating();
        self.transform_setup_generation = self.transform_setup_generation.wrapping_add(1);
        let generation = self.transform_setup_generation;
        let relationship = match self.doc.plan_pixel_transform(id) {
            Ok(plan) => plan,
            Err(_) => return false,
        };

        if self.has_selection() && self.selection_pixel_bounds().is_none() {
            let bounds = self.selection_cpu_cache().and_then(|data| {
                crate::mask::pixel_bounds_r8(data, self.doc.width, self.doc.height).map(
                    |[x, y, w, h]| crate::coord::WindowRect::from_xywh(x as i32, y as i32, w, h),
                )
            });
            if let Some(bounds) = bounds {
                self.set_selection_pixel_bounds(Some(bounds));
            } else {
                self.pending_transform = Some(PendingTransform {
                    node_id: id,
                    setup_generation: generation,
                    relationship,
                });
                return false;
            }
        }

        if self.prepare_transform_session(id, generation, relationship.clone()) {
            true
        } else {
            self.pending_transform = Some(PendingTransform {
                node_id: id,
                setup_generation: generation,
                relationship,
            });
            false
        }
    }

    pub(crate) fn prepare_transform_session(
        &mut self,
        initiator_id: LayerId,
        setup_generation: u64,
        relationship: TransformRelationshipSnapshot,
    ) -> bool {
        if setup_generation != self.transform_setup_generation
            || !self
                .doc
                .validate_pixel_transform_snapshot(initiator_id, &relationship)
        {
            return false;
        }
        let selection = self
            .selection_pixel_bounds()
            .map(|bounds| bounds.to_canvas(self.doc.canvas_origin));
        let has_selection = self.has_selection();
        if !has_selection {
            let mut waiting = false;
            for &node_id in &relationship.targets {
                let semantics = self
                    .doc
                    .pixel_transform_semantics(node_id)
                    .expect("relationship planner validates semantics");
                if semantics.bounds_policy
                    == crate::document::PixelTransformBoundsPolicy::AlphaContent
                    && self.compositor.content_bounds(node_id).is_none()
                {
                    self.compositor.request_content_bounds(
                        &self.gpu.device,
                        &self.gpu.queue,
                        node_id,
                    );
                    waiting = true;
                }
            }
            if waiting {
                return false;
            }
        }
        let mut geometries = Vec::with_capacity(relationship.targets.len());
        for &node_id in &relationship.targets {
            let Some(document_bounds) = self.doc.node_pixel_bounds(node_id) else {
                return false;
            };
            let semantics = self
                .doc
                .pixel_transform_semantics(node_id)
                .expect("relationship planner validates semantics");
            let extraction = if let Some(selection) = selection {
                document_bounds.intersect(selection)
            } else {
                match semantics.bounds_policy {
                    crate::document::PixelTransformBoundsPolicy::DocumentExtent => {
                        Some(document_bounds)
                    }
                    crate::document::PixelTransformBoundsPolicy::AlphaContent => {
                        let [x, y, width, height] = self
                            .compositor
                            .content_bounds(node_id)
                            .expect("all alpha-content bounds were resolved above");
                        if width == 0 || height == 0 {
                            None
                        } else {
                            let origin = self
                                .compositor
                                .node_texture(node_id)
                                .map(|texture| {
                                    texture.layer_to_canvas(crate::coord::LayerPoint::new(x, y))
                                })
                                .unwrap_or(crate::coord::CanvasPoint::new(x as i32, y as i32));
                            document_bounds
                                .intersect(CanvasRect::from_xywh(origin.x, origin.y, width, height))
                        }
                    }
                }
            };
            geometries.push((node_id, document_bounds, extraction));
        }
        if geometries
            .iter()
            .all(|(_, _, extraction)| extraction.is_none())
        {
            return false;
        }
        let operation_frame = if let Some(selection) = selection {
            selection
        } else {
            geometries
                .iter()
                .filter_map(|(_, _, extraction)| *extraction)
                .reduce(|left, right| left.union(right))
                .expect("at least one extraction")
        };

        let mut gpu_targets = Vec::new();
        let mut targets = Vec::new();
        for (node_id, document_bounds, extraction) in geometries {
            let Some(extraction_bounds) = extraction else {
                continue;
            };
            let source_origin = (extraction_bounds.x0(), extraction_bounds.y0());
            let prepared = self.gpu.encode_ret("transform-prepare-target", |encoder| {
                self.compositor.prepare_transform_target_from_gpu(
                    &self.gpu.device,
                    &self.gpu.queue,
                    encoder,
                    source_origin,
                    extraction_bounds.width,
                    extraction_bounds.height,
                    node_id,
                )
            });
            let Some(prepared) = prepared else {
                return false;
            };
            if has_selection {
                let local = (
                    extraction_bounds.x0() - self.doc.canvas_origin.x,
                    extraction_bounds.y0() - self.doc.canvas_origin.y,
                );
                if let Some(mask) = self.upload_cropped_selection_r8(
                    local,
                    extraction_bounds.width,
                    extraction_bounds.height,
                ) {
                    let source = GpuPaintTarget::from_canvas_texture(
                        &prepared.source_texture,
                        &prepared.source_view,
                        prepared.target_format,
                        CanvasRect::from_xywh(
                            0,
                            0,
                            extraction_bounds.width,
                            extraction_bounds.height,
                        ),
                    );
                    self.gpu.encode("transform-mask-source", |encoder| {
                        source.multiply_by_mask(
                            encoder,
                            &self.paint_pipelines,
                            &self.gpu.queue,
                            &mask,
                        );
                    });
                }
            }
            let clear_shape = if has_selection {
                ClearShape::Selection {
                    mask_bind_group: self.snapshot_selection_for_clear(),
                    uncovered: self
                        .doc
                        .pixel_transform_semantics(node_id)
                        .unwrap()
                        .uncovered_value,
                }
            } else {
                ClearShape::Rect(extraction_bounds)
            };
            targets.push(TransformTarget {
                node_id,
                document_bounds,
                extraction_bounds,
                clear_shape,
            });
            gpu_targets.push(prepared);
        }
        if setup_generation != self.transform_setup_generation {
            return false;
        }
        self.compositor
            .install_transform_session(&self.gpu.queue, gpu_targets);
        self.transform_session = Some(TransformSession {
            initiator_id,
            operation_frame,
            operation: Transform::identity(),
            relationship,
            targets,
            setup_generation,
            preview_revision: 0,
            has_selection,
        });
        if has_selection {
            self.clear_channel_overlay(crate::engine::OverlayChannel::Selection);
        }
        self.publish_transform_preview()
    }

    fn publish_transform_preview(&mut self) -> bool {
        let Some(mut session) = self.transform_session.take() else {
            return false;
        };
        session.preview_revision = session.preview_revision.wrapping_add(1);
        let revision = session.preview_revision;
        let params: Vec<_> = session
            .targets
            .iter()
            .map(
                |target| crate::gpu::floating_preview::TransformPreviewParams {
                    node_id: target.node_id,
                    matrix: crate::transform::evaluator_for_target(
                        &session.operation,
                        session.operation_frame,
                        target.extraction_bounds.origin,
                    ),
                    source_origin: (target.extraction_bounds.x0(), target.extraction_bounds.y0()),
                    source_width: target.extraction_bounds.width,
                    source_height: target.extraction_bounds.height,
                    clear_shape: &target.clear_shape,
                },
            )
            .collect();
        let published = self.compositor.publish_transform_preview_batch(
            &self.gpu.device,
            &self.gpu.queue,
            &self.paint_pipelines,
            revision,
            &params,
        );
        self.transform_session = Some(session);
        published
    }

    fn update_floating_preview(&mut self) {
        let Some(fc) = self.floating.as_ref() else {
            return;
        };
        self.compositor.update_floating_preview(
            &self.gpu.device,
            &self.gpu.queue,
            &self.paint_pipelines,
            &fc.transform.to_projective(),
            fc.source_origin,
            fc.source_width,
            fc.source_height,
            None,
        );
    }

    /// Snapshot the live GPU selection into a fresh canvas-sized R8 texture
    /// and return a paint-pipeline bind group sampling it. The returned
    /// bind group keeps the underlying texture alive for its lifetime, so
    /// it remains valid after the selection clear zeroes the live selection
    /// at the end of `setup_transform`.
    fn snapshot_selection_for_clear(&self) -> wgpu::BindGroup {
        let full = crate::coord::WindowRect::from_xywh(0, 0, self.doc.width, self.doc.height);
        self.selection_region_bind_group(full, wgpu::FilterMode::Linear)
            .expect("snapshot_selection_for_clear: selection_state allocated")
    }

    /// Update the floating content's transform and rebuild the per-frame
    /// preview texture so the host's blend reads the new shape. Accepts a full
    /// [`Transform`] so a `Perspective` switch (right-click) and per-drag
    /// homography updates flow through the same path as affine drags.
    #[handler]
    pub fn update_floating_matrix(&mut self, transform: Transform) {
        if let Some(session) = self.transform_session.as_mut() {
            session.operation = transform;
            self.publish_transform_preview();
        } else if let Some(fc) = self.floating.as_mut() {
            fc.transform = transform;
            self.update_floating_preview();
        } else {
            return;
        }
        self.compositor.mark_dirty();
    }

    fn commit_transform_session(&mut self) -> bool {
        let Some(session) = self.transform_session.take() else {
            return false;
        };
        if session.operation.is_identity() {
            self.compositor.clear_transform_session();
            if let Some(selection) = self.selection_cpu_cache().map(<[u8]>::to_vec) {
                self.update_selection_overlay_from_readback(selection);
            }
            return true;
        }
        if session.setup_generation != self.transform_setup_generation
            || !self
                .doc
                .validate_pixel_transform_snapshot(session.initiator_id, &session.relationship)
        {
            self.transform_session = Some(session);
            return false;
        }

        let proposed: Vec<_> = session
            .targets
            .iter()
            .map(|target| {
                let affected = crate::transform::affected_bounds(
                    &session.operation,
                    session.operation_frame,
                    target.extraction_bounds,
                );
                (target.node_id, target.document_bounds.union(affected))
            })
            .collect();

        // Prepare exact pre-publication undo snapshots. These schedule only
        // GPU→CPU copies; live textures and document bounds remain unchanged.
        let mut entries = Vec::with_capacity(session.targets.len());
        for target in &session.targets {
            let Some(texture) = self.compositor.node_texture(target.node_id) else {
                self.transform_session = Some(session);
                return false;
            };
            let format = texture.format();
            let frame = texture.canvas_frame();
            let snapshot = self.gpu.encode_ret("transform-stage-undo-save", |encoder| {
                self.region_scratch.save_region(
                    &self.gpu.device,
                    encoder,
                    &frame,
                    format,
                    target.document_bounds,
                )
            });
            let entry = commit_undo_region(
                &self.gpu,
                &self.region_scratch,
                &mut self.readbacks,
                "transform-stage-undo-entry",
                target.node_id,
                &frame,
                &snapshot,
                target.document_bounds,
            );
            entries.push(entry);
        }

        let selection_metadata = session.has_selection.then(|| {
            SelectionMetadataAction::new(
                self.selection_pixel_bounds(),
                self.selection_cpu_cache().map(<[u8]>::to_vec),
            )
        });
        let selection_action = if session.has_selection {
            let bounds = self
                .selection_pixel_bounds()
                .map(|bounds| {
                    CanvasRect::from_xywh(bounds.x0(), bounds.y0(), bounds.width, bounds.height)
                })
                .unwrap_or_else(|| CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height));
            self.save_selection_for_undo(bounds);
            self.commit_selection_undo_entry(bounds)
                .map(|entry| SelectionAction::new(true, entry))
        } else {
            None
        };
        if session.has_selection && selection_action.is_none() {
            self.transform_session = Some(session);
            return false;
        }

        // Allocate, copy, clear, and transform every replacement in one encoder.
        // Dropping this encoder on any error publishes no writes.
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("transform-staged-commit"),
            });
        let mut staged = Vec::with_capacity(session.targets.len());
        for (_target_index, (target, (_, extent))) in
            session.targets.iter().zip(&proposed).enumerate()
        {
            #[cfg(feature = "testing")]
            if self.transform_commit_fail_target == Some(_target_index) {
                self.transform_session = Some(session);
                return false;
            }
            let Some(resource) = self.compositor.prepare_staged_node_texture(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                target.node_id,
                *extent,
            ) else {
                self.transform_session = Some(session);
                return false;
            };
            let param = crate::gpu::floating_preview::TransformPreviewParams {
                node_id: target.node_id,
                matrix: crate::transform::evaluator_for_target(
                    &session.operation,
                    session.operation_frame,
                    target.extraction_bounds.origin,
                ),
                source_origin: (target.extraction_bounds.x0(), target.extraction_bounds.y0()),
                source_width: target.extraction_bounds.width,
                source_height: target.extraction_bounds.height,
                clear_shape: &target.clear_shape,
            };
            if !self.compositor.encode_transform_to_staged(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &self.paint_pipelines,
                &resource,
                &param,
            ) {
                self.transform_session = Some(session);
                return false;
            }
            staged.push(resource);
        }

        // Publication: one GPU submission followed by one non-interleavable CPU
        // mutation scope for mappings, authoritative extents, selection, history.
        self.gpu.queue.submit([encoder.finish()]);
        for &(node_id, extent) in &proposed {
            self.doc.set_node_pixel_bounds(node_id, extent);
        }
        self.compositor
            .publish_staged_node_textures(&self.gpu.device, &self.gpu.queue, staged);
        if session.has_selection {
            let bounds = self.selection_pixel_bounds();
            if let Some(selection) = self.compositor.selection_state_mut() {
                selection.clear_region(&self.gpu.queue, bounds);
            }
            self.set_selection_active(false);
            self.set_selection_pixel_bounds(None);
            self.invalidate_selection_cpu_cache();
            self.clear_channel_overlay(crate::engine::OverlayChannel::Selection);
        }

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::new();
        for ((target, (_, extent)), entry) in session.targets.iter().zip(&proposed).zip(entries) {
            if target.document_bounds != *extent {
                actions.push(Box::new(PixelBoundsAction::new(
                    target.node_id,
                    target.document_bounds,
                )));
            }
            actions.push(Box::new(GpuRegionAction::new(entry)));
        }
        if let Some(metadata) = selection_metadata {
            actions.push(Box::new(metadata));
        }
        if let Some(selection) = selection_action {
            actions.push(Box::new(selection));
        }
        self.push_undo(Box::new(CompoundAction::new(actions)));
        self.compositor.clear_transform_session();
        true
    }

    /// Commit floating content: render transformed pixels into the target
    /// layer/mask texture via a GPU render pass.
    #[handler]
    pub fn commit_floating(&mut self) {
        if self.transform_session.is_some() {
            self.commit_transform_session();
            return;
        }
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => {
                if self.pending_transform.take().is_some() {
                    self.transform_setup_generation =
                        self.transform_setup_generation.wrapping_add(1);
                }
                return;
            }
        };

        let layer_id = fc.target_layer;
        // The target can become locked after `begin_transform` / paste — fall
        // back to cancel-equivalent behavior (drop float state, no write to
        // the layer). The float is already taken out of `self.floating` above.
        if !self.doc.is_node_editable(layer_id) {
            self.compositor.clear_floating_content();
            self.compositor.mark_dirty();
            return;
        }
        // Format comes from the unified node-texture pool. Both raster layer
        // (RGBA8) and mask filter (R8) targets resolve through the same call.
        let format = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.format())
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

        let old_bounds = self.doc.node_pixel_bounds(layer_id);

        // Grow the target (or its host, for mask filters) so the layer
        // texture can hold any portion of the affected rect that lies
        // outside its current bounds — including pixels past the canvas
        // edge. Best-effort: if growth is refused (cap, or target is
        // neither raster nor filter with a raster host), commit falls
        // back to the pre-grow extent and the texture-side clip below
        // still keeps the commit consistent.
        let grew = self.grow_node_to_fit(layer_id, affected_canvas).is_some();

        // Path A — paste onto a layer auto-created for this paste.
        // The layer is empty by construction, so a single LayerAddAction
        // captures the whole paste as one undo step (no GpuRegionAction).
        let FloatingMode::Paste { created_layer_id } = fc.mode;
        if created_layer_id.is_some() {
            self.gpu.encode("paste-commit", |encoder| {
                self.compositor.commit_floating_to_texture(
                    &self.gpu.device,
                    encoder,
                    &self.gpu.queue,
                    &fc.transform.to_projective(),
                    fc.source_origin,
                    fc.source_width,
                    fc.source_height,
                );
            });

            let parent = self.doc.parent_of(layer_id);
            let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);
            self.push_undo(Box::new(LayerAddAction::new(layer_id, parent, pos)));

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
            Some(extent) if grew => extent,
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
            self.region_scratch.save_region(
                &self.gpu.device,
                encoder,
                &layer_frame!(),
                format,
                affected_canvas_rect,
            )
        });

        // The undo entry has to be committed BEFORE the ClearShape + commit
        // pass writes to the target, because `commit_region` reads from the
        // scratch (which holds the pre-clear state). Separate encode-and-
        // submit so the clear/commit pass sees the scratch upload done.
        let frame = layer_frame!();
        let entry = commit_undo_region(
            &self.gpu,
            &self.region_scratch,
            &mut self.readbacks,
            "transform-commit-undo",
            layer_id,
            &frame,
            &commit_snap,
            affected_canvas_rect,
        );

        self.gpu.encode("paste-commit", |encoder| {
            self.compositor.commit_floating_to_texture(
                &self.gpu.device,
                encoder,
                &self.gpu.queue,
                &fc.transform.to_projective(),
                fc.source_origin,
                fc.source_width,
                fc.source_height,
            );
        });
        let mut actions: Vec<Box<dyn UndoAction>> = Vec::new();
        if grew {
            if let Some(old_bounds) = old_bounds {
                actions.push(Box::new(PixelBoundsAction::new(layer_id, old_bounds)));
            }
        }
        actions.push(Box::new(GpuRegionAction::new(entry)));
        self.push_undo(Box::new(CompoundAction::new(actions)));

        // Clean up GPU state
        self.compositor.mark_node_pixels_dirty(layer_id);
        self.compositor.clear_floating_content();
    }

    #[cfg(feature = "testing")]
    pub fn test_transform_target_ids(&self) -> Vec<LayerId> {
        self.transform_session
            .as_ref()
            .map(|session| {
                session
                    .targets
                    .iter()
                    .map(|target| target.node_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(feature = "testing")]
    pub fn test_transform_preview_revision(&self) -> Option<u64> {
        self.compositor.transform_preview_revision()
    }

    #[cfg(feature = "testing")]
    pub fn test_fail_transform_commit_at_target(&mut self, target: Option<usize>) {
        self.transform_commit_fail_target = target;
    }

    /// Cancel floating content: drop the floating session. The live target
    /// texture was never mutated during a transform (preview lives on a
    /// separate texture), so cancel is a pure session-state reset.
    #[handler]
    pub fn cancel_floating(&mut self) {
        self.transform_setup_generation = self.transform_setup_generation.wrapping_add(1);
        self.pending_transform = None;
        if self.transform_session.take().is_some() {
            self.compositor.clear_transform_session();
            if let Some(selection) = self.selection_cpu_cache().map(<[u8]>::to_vec) {
                self.update_selection_overlay_from_readback(selection);
            }
            self.compositor.mark_dirty();
            return;
        }
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        let FloatingMode::Paste { created_layer_id } = fc.mode;
        if let Some(id) = created_layer_id {
            // Paste auto-created a target layer; drop it silently. No undo
            // entry to maintain — `LayerAddAction` is only pushed on commit.
            self.doc.detach_for_undo(id);
            self.compositor.dispose_layer(id);
            self.compositor.mark_dirty();
        }
        // Paste onto an existing target never mutates the live texture before
        // commit, so cancel has nothing to restore.

        self.compositor.clear_floating_content();
        if let Some(selection) = self.selection_cpu_cache().map(<[u8]>::to_vec) {
            self.update_selection_overlay_from_readback(selection);
        }
        self.compositor.mark_dirty();
    }
}
