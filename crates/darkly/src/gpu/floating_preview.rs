//! Floating-content preview methods on [`Compositor`].
//!
//! The floating preview is a *derived view* of the target node's texture:
//! when a transform is active, the compositor maintains a per-target
//! preview texture rebuilt on every matrix update, holding "what the
//! target would look like if commit ran right now". The render walk's
//! `effective_node_view` and `effective_mask_bind_group` accessors swap
//! the live view / mask bind group for the preview equivalents when the
//! floating target is encountered, so the host's normal blend pass
//! renders the preview without any extra render pass — and isolation,
//! grouping, and other branch-free render concerns compose with it
//! automatically.
//!
//! The compositor exposes primitives (set/clear floating content, update
//! uniforms + preview, commit to live target). The engine drives them by
//! calling `update_floating_preview` after each matrix change and on
//! `setup_transform`.

use crate::gpu::compositor::{BlendUniforms, Compositor};
use crate::layer::LayerId;

pub struct TransformGpuSession {
    pub targets: Vec<crate::gpu::transform::TransformState>,
    pub published_preview_revision: u64,
}

pub struct TransformPreviewParams<'a> {
    pub node_id: LayerId,
    pub matrix: crate::gpu::transform::Mat3,
    pub source_origin: (i32, i32),
    pub source_width: u32,
    pub source_height: u32,
    pub clear_shape: &'a crate::gpu::transform::ClearShape,
}

impl TransformGpuSession {
    pub fn target(&self, node_id: LayerId) -> Option<&crate::gpu::transform::TransformState> {
        self.targets
            .iter()
            .find(|target| target.target_layer == node_id)
    }
}

impl Compositor {
    /// Allocate the per-target preview texture (and, when target is R8, a
    /// mask-shape bind group sampling it) plus the canvas-aligned blend
    /// uniform buffer the host's blend pass reads when this layer is the
    /// floating target.
    ///
    /// Preview is canvas-sized (not live-sized) so a translate that moves
    /// content past the live layer's bounding box still has room on the
    /// preview to write — clipped at canvas bounds, which is all the
    /// viewport renders anyway. Commit's `grow_node_to_fit` separately
    /// expands the live layer so off-canvas pixels survive the commit.
    fn allocate_preview_resources(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (
        wgpu::Texture,
        wgpu::TextureView,
        Option<wgpu::BindGroup>,
        wgpu::Buffer,
    ) {
        let preview = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("floating-preview"),
            size: wgpu::Extent3d {
                width: self.canvas_width.max(1),
                height: self.canvas_height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // COPY_SRC needed because `render_commit` runs
            // `copy_for_compositing` against the render target before its
            // shader pass — when the target is the preview texture, that
            // copy reads from preview.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = preview.create_view(&wgpu::TextureViewDescriptor::default());
        let mask_bg = if format == wgpu::TextureFormat::R8Unorm {
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("floating-preview-mask-bg"),
                layout: &self.blend_pipelines.mask_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            }))
        } else {
            None
        };
        let blend_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("floating-preview-blend-uniforms"),
            size: std::mem::size_of::<BlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        (preview, view, mask_bg, blend_uniform_buf)
    }

    /// Look up the target's format + dimensions. Falls back to canvas-sized
    /// RGBA8 when the node texture hasn't been allocated yet (paste-as-
    /// floating creates the layer before its texture).
    fn target_format_and_dims(&self, target_layer: LayerId) -> (wgpu::TextureFormat, u32, u32) {
        match self.node_textures.get(&target_layer) {
            Some(t) => {
                let ext = t.layer_extent();
                (t.format(), ext.width, ext.height)
            }
            None => (
                wgpu::TextureFormat::Rgba8Unorm,
                self.canvas_width,
                self.canvas_height,
            ),
        }
    }

    /// Set up floating content for GPU preview from straight-alpha RGBA
    /// pixel data (used by the paste paths). The target's preview texture
    /// is allocated alongside.
    pub fn set_floating_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba_data: &[u8],
        _source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
    ) {
        let (target_format, _tw, _th) = self.target_format_and_dims(target_layer);
        let (preview_texture, preview_view, preview_mask_bg, preview_blend_uniform_buf) =
            self.allocate_preview_resources(device, target_format);
        self.transform_pass.set_floating_content(
            device,
            queue,
            &self.sampler,
            rgba_data,
            source_width,
            source_height,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bg,
            preview_blend_uniform_buf,
        );
        // Seed the preview's blend uniforms from the live layer's cached
        // props now that the floating session is active.
        self.write_preview_blend_uniforms_if_active(queue, target_layer);
        self.mark_dirty();
    }

    /// Set floating content by copying directly from a node's GPU texture.
    /// GPU→GPU copy — no CPU tiles involved. Format dispatch (R8 mask vs RGBA
    /// layer) is driven by the texture's own format from the unified node pool.
    pub fn prepare_transform_target_from_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
        semantics: &'static crate::document::PixelTransformSemantics,
    ) -> Option<crate::gpu::transform::TransformState> {
        let layer = self.node_textures.get(&target_layer)?;
        let target_format = layer.format();
        if target_format != semantics.format {
            return None;
        }
        let (preview_texture, preview_view, preview_mask_bg, preview_blend_uniform_buf) =
            self.allocate_preview_resources(device, target_format);
        // Re-borrow `layer` after `allocate_preview_resources` — the helper
        // doesn't take `&mut self`, but rust-analyzer prefers the explicit
        // re-fetch over keeping the borrow live across the helper call.
        let layer = self
            .node_textures
            .get(&target_layer)
            .expect("layer present after preview allocation");
        self.transform_pass.set_floating_content_from_gpu(
            device,
            queue,
            encoder,
            &self.sampler,
            layer,
            source_origin,
            source_width,
            source_height,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bg,
            preview_blend_uniform_buf,
        );
        self.transform_pass.take_paste_state()
    }

    pub fn install_transform_session(
        &mut self,
        queue: &wgpu::Queue,
        targets: Vec<crate::gpu::transform::TransformState>,
    ) {
        self.transform_session = Some(TransformGpuSession {
            targets,
            published_preview_revision: 0,
        });
        let ids: Vec<_> = self
            .transform_session
            .as_ref()
            .unwrap()
            .targets
            .iter()
            .map(|target| target.target_layer)
            .collect();
        for id in ids {
            self.write_preview_blend_uniforms_if_active(queue, id);
        }
    }

    pub fn encode_transform_to_staged(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        paint_pipelines: &crate::gpu::paint_target::PaintPipelines,
        staged: &crate::gpu::compositor::StagedNodeTexture,
        param: &TransformPreviewParams<'_>,
    ) -> bool {
        let Some(state) = self
            .transform_session
            .as_ref()
            .and_then(|session| session.target(param.node_id))
        else {
            return false;
        };
        let extent = staged.canvas_extent();
        self.transform_pass.update_state_uniforms(
            queue,
            state,
            &param.matrix,
            param.source_origin,
            param.source_width,
            param.source_height,
            (extent.x0(), extent.y0()),
            extent.width,
            extent.height,
            self.canvas_width,
            self.canvas_height,
        );
        let target = crate::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
            staged.texture(),
            staged.view(),
            state.target_format,
            extent,
        );
        match param.clear_shape {
            crate::gpu::transform::ClearShape::Rect(rect) => {
                target.clear_rect(encoder, paint_pipelines, queue, *rect)
            }
            crate::gpu::transform::ClearShape::Selection {
                mask_bind_group,
                uncovered,
            } => target.clear_with_selection(
                encoder,
                paint_pipelines,
                queue,
                mask_bind_group,
                *uncovered,
            ),
        }
        self.transform_pass.render_state_commit(
            device,
            encoder,
            state,
            staged.texture(),
            staged.view(),
        );
        true
    }

    pub fn clear_transform_session(&mut self) {
        self.transform_session = None;
        self.mark_dirty();
    }

    pub(super) fn transform_preview_target(
        &self,
        node_id: LayerId,
    ) -> Option<&crate::gpu::transform::TransformState> {
        self.transform_session
            .as_ref()
            .filter(|session| session.published_preview_revision > 0)
            .and_then(|session| session.target(node_id))
            .or_else(|| {
                self.transform_pass
                    .paste
                    .as_ref()
                    .filter(|target| target.target_layer == node_id)
            })
    }

    pub fn transform_preview_revision(&self) -> Option<u64> {
        self.transform_session
            .as_ref()
            .map(|session| session.published_preview_revision)
    }

    pub fn publish_transform_preview_batch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paint_pipelines: &crate::gpu::paint_target::PaintPipelines,
        revision: u64,
        params: &[TransformPreviewParams<'_>],
    ) -> bool {
        let Some(session) = self.transform_session.as_ref() else {
            return false;
        };
        if params.len() != session.targets.len()
            || params
                .iter()
                .zip(&session.targets)
                .any(|(param, target)| param.node_id != target.target_layer)
        {
            return false;
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("transform-preview-batch"),
        });
        let origin = self.canvas_origin;
        let canvas_rect = self.canvas_rect();
        for (param, state) in params.iter().zip(&session.targets) {
            let Some(live) = self.node_textures.get(&param.node_id) else {
                return false;
            };
            self.transform_pass.update_state_uniforms(
                queue,
                state,
                &param.matrix,
                param.source_origin,
                param.source_width,
                param.source_height,
                (origin.x, origin.y),
                self.canvas_width,
                self.canvas_height,
                self.canvas_width,
                self.canvas_height,
            );
            crate::gpu::clear_view_transparent(
                &mut encoder,
                &state.preview_view,
                "transform-preview-clear",
            );
            if let Some(visible) = live.canvas_extent().intersect(canvas_rect) {
                let local = live
                    .canvas_to_layer_rect(visible)
                    .expect("intersection is layer-local");
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: live.texture(),
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: local.x0(),
                            y: local.y0(),
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &state.preview_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: (visible.x0() - origin.x) as u32,
                            y: (visible.y0() - origin.y) as u32,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: visible.width,
                        height: visible.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            let preview_target = crate::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
                &state.preview_texture,
                &state.preview_view,
                state.target_format,
                canvas_rect,
            );
            match param.clear_shape {
                crate::gpu::transform::ClearShape::Rect(rect) => {
                    preview_target.clear_rect(&mut encoder, paint_pipelines, queue, *rect);
                }
                crate::gpu::transform::ClearShape::Selection {
                    mask_bind_group,
                    uncovered,
                } => preview_target.clear_with_selection(
                    &mut encoder,
                    paint_pipelines,
                    queue,
                    mask_bind_group,
                    *uncovered,
                ),
            }
            self.transform_pass.render_state_commit(
                device,
                &mut encoder,
                state,
                &state.preview_texture,
                &state.preview_view,
            );
        }
        queue.submit([encoder.finish()]);
        self.transform_session
            .as_mut()
            .expect("session retained during batch")
            .published_preview_revision = revision;
        self.mark_dirty();
        true
    }

    /// Update the floating preview: write the current matrix uniforms, copy
    /// live target → preview texture, apply the engine-side `clear_shape`
    /// (None for paste mode, `Some` for transform mode), then run the
    /// commit shader into the preview. The resulting preview texture is
    /// what the host's blend pass reads through `effective_node_view` /
    /// `effective_mask_bind_group` for the rest of the frame.
    ///
    /// Called by the engine on `setup_transform` (initial preview) and
    /// `update_floating_matrix` (per drag tick).
    #[allow(clippy::too_many_arguments)]
    pub fn update_floating_preview(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paint_pipelines: &crate::gpu::paint_target::PaintPipelines,
        matrix: &crate::gpu::transform::Mat3,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        clear_shape: Option<&crate::gpu::transform::ClearShape>,
    ) {
        let Some(state) = self.transform_pass.paste.as_ref() else {
            return;
        };
        let live = match self.node_textures.get(&state.target_layer) {
            Some(t) => t,
            None => return,
        };

        // The preview is a window-local surface anchored at `canvas_origin`:
        // its texel (0,0) maps to plane `canvas_origin` (that is how the host's
        // blend composites it — `layer_offset = canvas_origin` in
        // `write_preview_blend_uniforms_if_active`). So the transform shader's
        // `target_offset` — "where on the canvas the target's pixels live" —
        // must be `canvas_origin`, not (0,0); otherwise source content drawn at
        // plane `C` lands in texel `C` and then composites at plane
        // `canvas_origin + C`, shifting the whole preview by the crop offset.
        let origin = self.canvas_origin;
        self.transform_pass.update_uniforms(
            queue,
            matrix,
            source_origin,
            source_width,
            source_height,
            (origin.x, origin.y),
            self.canvas_width,
            self.canvas_height,
            self.canvas_width,
            self.canvas_height,
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("floating-preview-build"),
        });

        // 0. Reset the whole preview to transparent. The copy below only
        //    repaints the canvas portion live actually covers, and the
        //    commit shader discards outside the transformed source bounds —
        //    so canvas pixels outside live's extent would otherwise retain
        //    previous-frame transform writes (ghost trails).
        crate::gpu::clear_view_transparent(
            &mut encoder,
            &state.preview_view,
            "floating-preview-clear",
        );

        // 1. Copy live → preview so non-source-rect pixels are preserved.
        //    Live texture sits at its plane extent; clip the copy to the
        //    on-canvas (window) portion. The preview is window-local anchored at
        //    `canvas_origin`, so intersect against the plane canvas window
        //    (`canvas_rect()`, at `canvas_origin`) and write to the window-local
        //    destination texel `visible - canvas_origin`. Off-window pixels are
        //    not in the preview — the viewport never renders them, and commit's
        //    `grow_node_to_fit` preserves them on the live texture.
        let canvas_rect = self.canvas_rect();
        let live_canvas_extent = live.canvas_extent();
        if let Some(visible) = live_canvas_extent.intersect(canvas_rect) {
            // Translate the visible canvas slice into the live texture's
            // layer-local coordinate frame for the GPU copy origin.
            let visible_layer = live
                .canvas_to_layer_rect(visible)
                .expect("intersect with live's extent yields a layer-local rect");
            let dst_x = (visible.x0() - origin.x) as u32;
            let dst_y = (visible.y0() - origin.y) as u32;
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: live.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: visible_layer.x0(),
                        y: visible_layer.y0(),
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &state.preview_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_x,
                        y: dst_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: visible.width,
                    height: visible.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // 2. Apply the source-rect clear (transform mode only — paste mode
        //    leaves the preview as a copy of the live target so the commit
        //    shader composites over the existing pixels). The preview is
        //    canvas-aligned, so the paint target reports canvas dims/offset.
        if let Some(cs) = clear_shape {
            let preview_target = crate::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
                &state.preview_texture,
                &state.preview_view,
                state.target_format,
                self.canvas_rect(),
            );
            match cs {
                crate::gpu::transform::ClearShape::Rect(rect) => {
                    preview_target.clear_rect(&mut encoder, paint_pipelines, queue, *rect);
                }
                crate::gpu::transform::ClearShape::Selection {
                    mask_bind_group,
                    uncovered,
                } => {
                    preview_target.clear_with_selection(
                        &mut encoder,
                        paint_pipelines,
                        queue,
                        mask_bind_group,
                        *uncovered,
                    );
                }
            }
        }

        // 3. Run the commit shader into the preview at the current matrix.
        self.transform_pass.render_commit(
            device,
            &mut encoder,
            &state.preview_texture,
            &state.preview_view,
        );

        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Render the transform directly into the live target texture.
    /// Used by `commit_floating()` after the engine has applied the
    /// `clear_shape` to the live target.
    pub fn commit_floating_to_texture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        matrix: &crate::gpu::transform::Mat3,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
    ) {
        let Some(state) = self.transform_pass.paste.as_ref() else {
            return;
        };
        let live = match self.node_textures.get(&state.target_layer) {
            Some(t) => t,
            None => return,
        };

        let live_extent = live.canvas_extent();
        self.transform_pass.update_uniforms(
            queue,
            matrix,
            source_origin,
            source_width,
            source_height,
            (live_extent.x0(), live_extent.y0()),
            live_extent.width,
            live_extent.height,
            self.canvas_width,
            self.canvas_height,
        );

        self.transform_pass
            .render_commit(device, encoder, live.texture(), live.view());
    }

    /// Remove floating content GPU state.
    pub fn clear_floating_content(&mut self) {
        self.transform_pass.clear();
        self.mark_dirty();
    }

    /// Get a reference to the transform source texture and its view.
    /// Returns None if no floating content is active.
    pub fn transform_source_texture(&self) -> Option<(&wgpu::Texture, &wgpu::TextureView)> {
        self.transform_pass
            .paste
            .as_ref()
            .map(|s| (&s.source_texture, &s.source_view))
    }

    /// Write the floating preview's canvas-aligned blend uniforms using the
    /// given layer's cached blend props. No-op when there is no active
    /// floating, or when the active floating's target is not `layer_id`.
    /// Called from both `update_layer_uniforms` (on prop change) and the
    /// floating setup paths (to seed the buffer at session start).
    pub(super) fn write_preview_blend_uniforms_if_active(
        &self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
    ) {
        let state = match self
            .transform_session
            .as_ref()
            .and_then(|session| session.target(layer_id))
            .or_else(|| {
                self.transform_pass
                    .paste
                    .as_ref()
                    .filter(|s| s.target_layer == layer_id)
            }) {
            Some(state) => state,
            None => return,
        };
        let cache = match self.layer_cache.get(&layer_id) {
            Some(c) => c,
            None => return,
        };
        let uniforms = BlendUniforms {
            opacity: cache.opacity,
            blend_mode: cache.blend_mode,
            isolated: cache.isolated as u32,
            _pad1: 0.0,
            // The preview texture is window-sized and window-anchored, so it
            // composites as a "layer" occupying the canvas window in the plane:
            // `layer_offset = canvas_origin`, `layer_size = canvas_size` makes
            // the shared-canvas plane round-trip collapse to identity.
            layer_offset: [self.canvas_origin.x as f32, self.canvas_origin.y as f32],
            layer_size: [self.canvas_width as f32, self.canvas_height as f32],
        };
        queue.write_buffer(
            &state.preview_blend_uniform_buf,
            0,
            bytemuck::bytes_of(&uniforms),
        );
    }
}
