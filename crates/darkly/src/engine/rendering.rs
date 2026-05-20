//! Rendering, view transform, thumbnails, undo/redo, and async readback polling.

use super::{DarklyEngine, ReadbackContext};
use crate::gpu::atlas::CanvasFrame;
use crate::gpu::context::GpuContext;
use crate::gpu::readback;
use crate::gpu::region_store::{RegionStore, UndoRegionEntry};
use crate::gpu::view::ViewTransform;
use crate::layer::LayerId;
use crate::undo::GpuRegionAction;

/// Thumbnail size used for the layer panel previews. Single source of
/// truth — the frontend reads it via `engine_default_thumb_size()` so
/// the auto-queued readbacks land in the cache at the same dimensions
/// the panel renders. Don't drift the literal in `thumbnails.ts`.
pub const DEFAULT_THUMB_SIZE: u32 = 36;

impl DarklyEngine {
    // --- View transform ---

    pub fn set_view_transform(
        &mut self,
        pan_x: f32,
        pan_y: f32,
        zoom: f32,
        rotation: f32,
        mirror_h: bool,
        screen_w: f32,
        screen_h: f32,
    ) {
        let transform = ViewTransform::from_pan_zoom_rotate(
            pan_x,
            pan_y,
            zoom,
            rotation,
            mirror_h,
            screen_w,
            screen_h,
            self.doc.width as f32,
            self.doc.height as f32,
        );
        self.view_transform = transform;
        self.compositor
            .update_view_transform(&self.gpu.queue, &transform);
        self.compositor.mark_needs_present();
    }

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        self.view_transform.screen_to_canvas(screen_x, screen_y)
    }

    /// Push the workspace background color (the area shown outside the
    /// canvas rectangle by the present shader). Frontend calls this on
    /// theme change with the resolved `--canvas-bg` CSS value.
    pub fn set_viewport_bg(&mut self, bg: [f32; 4]) {
        self.compositor.set_viewport_bg(&self.gpu.queue, bg);
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
            // Composited texture is canvas-aligned: canvas coords == texture
            // coords here, so a single-pixel layer rect at (px, py) names the
            // same pixel either way.
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                texture,
                wgpu::TextureFormat::Rgba8Unorm,
                crate::coord::LayerRect::from_xywh(px, py, 1, 1),
            );
            self.readbacks.submit(request, ReadbackContext::ColorPick);
        });

        // Return cached color for immediate feedback — real result arrives next frame.
        self.last_picked_color
    }

    // --- Thumbnails ---

    /// Return the cached thumbnail for any node id (raster layer or mask
    /// modifier). Pure read — readback queueing is owned by
    /// `drain_dirty_thumbnail_readbacks` (driven by `mark_node_pixels_dirty`
    /// at every pixel-write site). Auto-queueing from this getter would
    /// create a feedback loop with the JS-side `thumbnailEpoch` sync.
    pub fn node_thumbnail(&self, node_id: LayerId, thumb_w: u32, thumb_h: u32) -> Vec<u8> {
        self.thumbnail_cache
            .get(node_id)
            .cloned()
            .unwrap_or_else(|| vec![0u8; (thumb_w * thumb_h * 4) as usize])
    }

    /// Kick off an async GPU readback for a thumbnail of any node by id,
    /// if one isn't already pending. Format is derived from the node's
    /// GPU texture — callers don't dispatch on layer-vs-modifier.
    ///
    /// Reads the texture's full extent, not a canvas-sized rect. Layer
    /// textures may be smaller than canvas (e.g. a small paste) or larger
    /// (off-canvas paste, chunk-aligned growth past canvas edge); copying
    /// `[0, 0, canvas_w, canvas_h]` would over-read in either case and
    /// fail wgpu validation. The source dims are carried to the completion
    /// handler so the downscale uses the same layout the buffer holds.
    fn request_thumbnail_readback(&mut self, node_id: LayerId, thumb_w: u32, thumb_h: u32) {
        if self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::Thumbnail { node_id: id, .. } if *id == node_id))
        {
            return;
        }

        let (texture, format, layer_rect) = match self.compositor.node_texture(node_id) {
            Some(t) => (t.texture(), t.format(), t.layer_extent()),
            None => return,
        };
        let tex_w = layer_rect.width;
        let tex_h = layer_rect.height;

        self.gpu.encode("thumb-readback", |encoder| {
            let request =
                readback::request_readback(&self.gpu.device, encoder, texture, format, layer_rect);
            self.readbacks.submit(
                request,
                ReadbackContext::Thumbnail {
                    node_id,
                    source_w: tex_w,
                    source_h: tex_h,
                    thumb_w,
                    thumb_h,
                },
            );
        });
    }

    // --- Rendering ---

    /// Poll all pending async readback operations.
    ///
    /// Called at the start of each frame. Returns true if any operation
    /// completed (and therefore the compositor should re-render).
    fn poll_pending(&mut self) -> bool {
        // Poll pending diff rect for deferred undo commit.
        if self.diff_rect.is_pending() {
            if let Some(result) = self.diff_rect.poll(&self.gpu.device) {
                if let Some(commit) = self.pending_undo_commit.take() {
                    if let Some(rect) = result {
                        if let Some(layer_frame) = self
                            .compositor
                            .node_texture(commit.layer_id)
                            .map(|t| t.canvas_frame())
                        {
                            let mut entry = None;
                            self.gpu.encode("brush-stroke-end", |encoder| {
                                entry = Some(self.region_store.commit_region(
                                    encoder,
                                    commit.layer_id,
                                    &layer_frame,
                                    &commit.snapshot,
                                    rect,
                                ));
                            });
                            if let Some(entry) = entry {
                                self.push_undo(Box::new(GpuRegionAction::new(entry)));
                            }
                        }
                    }
                    // else: textures identical, no undo entry needed.
                }
            }
        }

        // Poll content bounds compute readbacks.
        let bounds_completed = self.compositor.poll_content_bounds(&self.gpu.device);
        let mut any_completed = false;

        // Complete pending transform if content bounds just arrived.
        if let Some(pt) = &self.pending_transform {
            if bounds_completed.contains(&pt.node_id) {
                let node_id = pt.node_id;
                self.pending_transform = None;

                if self.floating.is_none() {
                    if let Some(bounds) = self.compositor.content_bounds(node_id) {
                        // content_bounds are layer-local; translate to canvas.
                        let [bx, by, bw, bh] = bounds;
                        let canvas_origin = self
                            .compositor
                            .node_texture(node_id)
                            .map(|t| t.layer_to_canvas(crate::coord::LayerPoint::new(bx, by)))
                            .unwrap_or(crate::coord::CanvasPoint::new(bx as i32, by as i32));
                        self.setup_transform(node_id, (canvas_origin.x, canvas_origin.y), bw, bh);
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
            self.handle_completed_readback(ctx, pixels);
        }
        true
    }

    /// Dispatch a completed readback to the appropriate handler. Shared
    /// between the frame-loop poll and the test-only flush so both paths
    /// honour every variant identically.
    pub(crate) fn handle_completed_readback(&mut self, ctx: ReadbackContext, pixels: Vec<u8>) {
        match ctx {
            ReadbackContext::FloodFill {
                node_id,
                seed_canvas,
                color,
                tolerance,
                extent,
            } => self.complete_flood_fill(node_id, seed_canvas, color, tolerance, extent, pixels),
            ReadbackContext::ColorPick => {
                if pixels.len() >= 4 {
                    self.last_picked_color = [pixels[0], pixels[1], pixels[2], pixels[3]];
                }
            }
            ReadbackContext::Copy {
                node_id,
                region,
                is_cut,
            } => {
                self.complete_copy(node_id, region, is_cut, pixels);
            }
            ReadbackContext::MagicWand {
                was_active,
                node_id,
                seed_canvas,
                tolerance,
                mode,
                extent,
            } => {
                self.complete_magic_wand(
                    was_active,
                    node_id,
                    seed_canvas,
                    tolerance,
                    mode,
                    extent,
                    pixels,
                );
            }
            ReadbackContext::ExportImage { width, height } => {
                self.complete_export(width, height, pixels);
            }
            ReadbackContext::SaveDocument {
                kind,
                width,
                height,
            } => {
                self.complete_save_readback(kind, width, height, pixels);
            }
            ReadbackContext::SelectionReadback => {
                self.update_selection_overlay_from_readback(pixels);
                // Resume deferred operations that were waiting for
                // selection cpu_cache / pixel_bounds.
                if let Some(pc) = self.pending_copy.take() {
                    self.start_copy_readback(pc.layer_id, pc.is_cut);
                }
                if self.selection_pixel_bounds().is_some() {
                    if let Some(pt) = self.pending_transform.take() {
                        if self.floating.is_none() {
                            self.begin_transform(pt.node_id);
                        }
                    }
                }
            }
            ReadbackContext::Thumbnail {
                node_id,
                source_w,
                source_h,
                thumb_w,
                thumb_h,
            } => {
                let is_r8 = self
                    .compositor
                    .node_texture(node_id)
                    .map(|t| t.format() == wgpu::TextureFormat::R8Unorm)
                    .unwrap_or(false);
                let thumb = if is_r8 {
                    generate_mask_thumbnail_from_pixels(
                        &pixels, source_w, source_h, thumb_w, thumb_h,
                    )
                } else {
                    generate_rgba_thumbnail_from_pixels(
                        &pixels, source_w, source_h, thumb_w, thumb_h,
                    )
                };
                self.thumbnail_cache.insert(node_id, thumb);
                // Tell the frontend "fresh thumbnail bytes available".
                self.thumbnail_version = self.thumbnail_version.wrapping_add(1);
            }
            ReadbackContext::BrushEditorPreview {
                width,
                height,
                graph_version,
            } => {
                // Drop stale results — if the graph has changed since
                // this render was issued, a fresher render has already
                // been queued and will supersede this one.
                if graph_version == self.brush_graph_version() {
                    let (tw, th) = super::brush_library::BRUSH_THUMBNAIL_SIZE;
                    let framed = frame_stroke_thumbnail(
                        &pixels,
                        width,
                        height,
                        tw,
                        th,
                        self.preview_theme_bg,
                    );
                    let png_bytes = encode_rgba_as_png(&framed, tw, th);
                    if !png_bytes.is_empty() {
                        self.brush_editor_preview_cache = Some(png_bytes);
                    }
                }
            }
            ReadbackContext::BrushThumbnailForSave {
                name,
                width,
                height,
            } => {
                let (tw, th) = super::brush_library::BRUSH_THUMBNAIL_SIZE;
                let framed =
                    frame_stroke_thumbnail(&pixels, width, height, tw, th, self.preview_theme_bg);
                let png_bytes = encode_rgba_as_png(&framed, tw, th);
                if !png_bytes.is_empty() {
                    self.brush_library.set_thumbnail(&name, png_bytes);
                }
            }
            ReadbackContext::BrushDabThumbnail {
                name,
                width,
                height,
            } => {
                let png_bytes = frame_dab_thumbnail(&pixels, width, height, self.preview_theme_bg);
                if !png_bytes.is_empty() {
                    self.brush_library.set_dab_thumbnail(&name, png_bytes);
                }
            }
            ReadbackContext::ActiveBrushDab { topology_version } => {
                // Drop stale results — but key off topology, not graph
                // version: scrub-only changes don't affect the rendered
                // dab thanks to `reset_exposed_scrubs`, so a readback
                // queued before a scrub change is still valid.
                if topology_version == self.brush_topology_version() {
                    let (w, h) = super::brush_library::BRUSH_DAB_RENDER_SIZE;
                    let png_bytes = frame_dab_thumbnail(&pixels, w, h, self.preview_theme_bg);
                    if !png_bytes.is_empty() {
                        self.active_dab_preview_cache = Some(png_bytes);
                    }
                }
            }
            ReadbackContext::NodePreview {
                node_id,
                topology_version,
            } => {
                // Drop stale results — same shape as `ActiveBrushDab`. If
                // the topology bumped between submit and now, the user has
                // changed the graph and another render is queued; this
                // result is for the old graph and would lie about the
                // current node output.
                if topology_version == self.brush_topology_version() {
                    let (w, h) = super::brush_library::BRUSH_DAB_RENDER_SIZE;
                    let png_bytes = frame_dab_thumbnail(&pixels, w, h, self.preview_theme_bg);
                    if !png_bytes.is_empty() {
                        self.node_preview_cache
                            .insert(node_id, (topology_version, png_bytes));
                    }
                }
            }
        }
    }

    /// Get the most recently picked color (updated asynchronously).
    pub fn last_picked_color(&self) -> [u8; 4] {
        self.last_picked_color
    }

    /// True if a color pick readback is still in flight.
    pub fn has_pending_color_pick(&self) -> bool {
        self.readbacks
            .any(|c| matches!(c, ReadbackContext::ColorPick))
    }

    /// Monotonic counter bumped each time a thumbnail readback lands in
    /// the cache. The frontend mirrors this into a Svelte-reactive epoch
    /// so the layer panel's `$derived` re-runs on async cache updates.
    pub fn thumbnail_version(&self) -> u32 {
        self.thumbnail_version
    }

    /// Drain layers whose pixels were modified since the last call and
    /// queue thumbnail readbacks at the engine's default panel size.
    /// Run on every `render()` (production *and* headless tests) so the
    /// layer panel sees fresh thumbnails after paint, fill, undo, paste
    /// — anything that calls `compositor.mark_layer_pixels_dirty`.
    fn drain_dirty_thumbnail_readbacks(&mut self) {
        let nodes = self.compositor.drain_dirty_pixels();
        for node_id in nodes {
            self.request_thumbnail_readback(node_id, DEFAULT_THUMB_SIZE, DEFAULT_THUMB_SIZE);
        }
    }

    /// Render a frame. Returns true if animations need another frame.
    pub fn render(&mut self, time_secs: f32) -> bool {
        // Sub-phase wall-clock timing for the slow-frame log. Always
        // recorded into `self.last_frame_phases` even on fast frames — the
        // WASM bridge decides whether to emit; nominal cost is a handful
        // of `Instant::now()` calls.
        let t_poll = web_time::Instant::now();
        let pending_completed = self.poll_pending();
        if pending_completed {
            self.compositor.mark_dirty();
        }
        let poll_us = t_poll.elapsed().as_micros() as u64;

        let t_thumb = web_time::Instant::now();
        // Auto-queue thumbnail readbacks for layers whose pixels were
        // modified since the last frame. Must run *before* the headless
        // early-return below so tests exercise the same code path the
        // production frame loop does.
        self.drain_dirty_thumbnail_readbacks();
        let thumb_us = t_thumb.elapsed().as_micros() as u64;

        // Headless mode (tests): poll pending ops but skip presentation.
        let (surface, surface_config) = match (&self.gpu.surface, &self.gpu.surface_config) {
            (Some(s), Some(c)) => (s, c),
            _ => {
                self.last_frame_phases = super::FrameRenderPhases {
                    poll_us,
                    thumb_us,
                    anim_us: 0,
                    compositor_us: 0,
                };
                return self.readbacks.has_pending()
                    || self.compositor.has_pending_content_bounds()
                    || self.diff_rect.is_pending();
            }
        };

        // Skip rendering when the surface has zero dimensions (e.g. canvas
        // squeezed to 0 height by a UI panel).  WebGPU cannot create
        // 0-dimension textures and attempting to do so corrupts the device.
        if surface_config.width == 0 || surface_config.height == 0 {
            self.last_frame_phases = super::FrameRenderPhases {
                poll_us,
                thumb_us,
                anim_us: 0,
                compositor_us: 0,
            };
            return self.readbacks.has_pending() || self.compositor.has_pending_content_bounds();
        }

        let t_anim = web_time::Instant::now();
        self.compositor
            .update_animations(&self.gpu.queue, time_secs, &self.doc);
        let anim_us = t_anim.elapsed().as_micros() as u64;

        let t_comp = web_time::Instant::now();
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            surface,
            surface_config,
            &mut self.doc,
        );
        let compositor_us = t_comp.elapsed().as_micros() as u64;

        self.last_frame_phases = super::FrameRenderPhases {
            poll_us,
            thumb_us,
            anim_us,
            compositor_us,
        };

        // Keep requesting frames while async operations are in flight.
        self.compositor.needs_animation(&self.doc)
            || self.readbacks.has_pending()
            || self.compositor.has_pending_content_bounds()
            || self.diff_rect.is_pending()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || self.gpu.is_headless() {
            return;
        }
        self.gpu.resize(width, height);
        self.compositor
            .veil_chain_mut()
            .resize(&self.gpu.device, &self.gpu.queue, width, height);
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
        // Force-flush any pending diff-based undo commit so the most recent
        // stroke's entry is on the stack before we pop.  Without this, an
        // undo fired in the same frame as end_stroke (or before poll_pending
        // runs) would skip the just-finished stroke entirely.
        self.flush_pending_undo_commit();

        let action = match direction {
            UndoDirection::Undo => self.undo_stack.pop_for_undo(),
            UndoDirection::Redo => self.undo_stack.pop_for_redo(),
        };
        let mut action = match action {
            Some(a) => a,
            None => return,
        };

        match direction {
            UndoDirection::Undo => {
                action.undo(&mut self.doc);
            }
            UndoDirection::Redo => {
                action.redo(&mut self.doc);
            }
        }

        // Sync layer/mask state BEFORE restoring GPU regions, so that mask
        // textures are (re)created if needed by the undo action.
        self.sync_compositor_layers();

        // If this is a GPU region action, execute the texture restore.
        // Node id resolves to the right texture via the unified node-texture
        // pool — caller no longer dispatches by format.
        if let Some(entry) = action.gpu_region_entry_mut() {
            let node_id = entry.layer_id;
            let frame = self
                .compositor
                .node_texture(node_id)
                .map(|t| t.canvas_frame());
            if let Some(frame) = frame {
                let label = match direction {
                    UndoDirection::Undo => "undo-restore",
                    UndoDirection::Redo => "redo-restore",
                };
                restore_gpu_region(&self.gpu, &mut self.region_store, entry, &frame, label);
                // Restored pixels — refresh the panel thumbnail.
                self.compositor.mark_node_pixels_dirty(node_id);
            }
        }

        // If this is a selection GPU action, restore the selection texture
        // and swap the active flag.
        if let Some(restored_active) = action.swap_selection_active(self.has_selection()) {
            self.set_selection_active(restored_active);

            if let Some(entry) = action.selection_region_entry_mut() {
                let frame = self.compositor.selection_state().map(|s| s.canvas_frame());
                if let Some(frame) = frame {
                    let label = match direction {
                        UndoDirection::Undo => "undo-sel-restore",
                        UndoDirection::Redo => "redo-sel-restore",
                    };
                    restore_gpu_region(&self.gpu, &mut self.region_store, entry, &frame, label);
                }
            }

            self.set_selection_pixel_bounds(None); // will be recomputed from readback
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
        let isolated = self.isolated_node;
        // The host's `isolated` uniform = "render my mask as grayscale on
        // canvas". That's what mask isolation means: the canvas becomes the
        // mask. When the target IS the host (raster/group itself), the host
        // renders normally — the isolation filter elsewhere already hides
        // its siblings. So this flag only fires when the target is a
        // modifier whose host is this node.
        let isolated_host = |node_id: LayerId| -> bool {
            match isolated {
                Some(t) => self.doc.modifiers_of(node_id).contains(&t),
                None => false,
            }
        };

        // --- Raster layers: ensure the GPU texture + uniforms ---
        struct RasterInfo {
            id: LayerId,
            opacity: f32,
            blend_mode_gpu: u32,
            isolated: bool,
            bounds: crate::coord::CanvasRect,
        }
        let infos: Vec<RasterInfo> = self
            .doc
            .all_raster_layers()
            .into_iter()
            .map(|r| RasterInfo {
                id: r.id,
                opacity: r.blend.opacity,
                blend_mode_gpu: r.blend.blend_mode.gpu_value,
                isolated: isolated_host(r.id),
                bounds: r.pixels.bounds,
            })
            .collect();

        for info in &infos {
            self.compositor.ensure_raster_layer(
                &self.gpu.device,
                &self.gpu.queue,
                info.id,
                info.bounds,
            );
            self.compositor.update_raster_uniforms_full(
                &self.gpu.queue,
                info.id,
                info.opacity,
                info.blend_mode_gpu,
                info.isolated,
            );
        }

        // --- Void layers: ensure the procedural texture + per-instance cache ---
        // Void state is regenerable from `(void_type, params)`, so on load
        // (or after `Compositor::recreate_resources`) we walk the doc and
        // rebuild any missing GPU caches. `ensure_void_layer` is idempotent.
        struct VoidInfo {
            id: LayerId,
            void_type: String,
            params: Vec<crate::gpu::params::ParamValue>,
            opacity: f32,
            blend_mode_gpu: u32,
            isolated: bool,
        }
        let void_infos: Vec<VoidInfo> = self
            .doc
            .all_void_layers()
            .into_iter()
            .map(|v| VoidInfo {
                id: v.id,
                void_type: v.void_type.clone(),
                params: v.params.clone(),
                opacity: v.blend.opacity,
                blend_mode_gpu: v.blend.blend_mode.gpu_value,
                isolated: isolated_host(v.id),
            })
            .collect();
        for info in &void_infos {
            self.compositor.ensure_void_layer(
                &self.gpu.device,
                &self.gpu.queue,
                info.id,
                &info.void_type,
                &info.params,
            );
            self.compositor.update_void_uniforms_full(
                &self.gpu.queue,
                info.id,
                info.opacity,
                info.blend_mode_gpu,
                info.isolated,
            );
        }

        // --- Mask modifiers: ensure the R8 GPU texture for any host with a mask ---
        // Also ensures the per-host passthrough snapshot+lerp resource so the
        // group composite branch can engage on the next frame; both are
        // idempotent and keyed against existence in the compositor's pools.
        struct MaskInfo {
            modifier_id: LayerId,
            host_id: LayerId,
            bounds: crate::coord::CanvasRect,
        }
        let mask_infos: Vec<MaskInfo> = self
            .doc
            .all_modifiers()
            .into_iter()
            .filter_map(|m| {
                let buf = m.pixels()?;
                if buf.format != wgpu::TextureFormat::R8Unorm {
                    return None;
                }
                let host_id = self.doc.parent_of(m.id)?;
                Some(MaskInfo {
                    modifier_id: m.id,
                    host_id,
                    bounds: buf.bounds,
                })
            })
            .collect();
        for info in mask_infos {
            if self.compositor.node_texture(info.modifier_id).is_none() {
                self.compositor.ensure_node_texture(
                    &self.gpu.device,
                    &self.gpu.queue,
                    info.modifier_id,
                    wgpu::TextureFormat::R8Unorm,
                    info.bounds,
                );
            }
            self.compositor
                .ensure_passthrough_mask_state(&self.gpu.device, info.host_id);
        }

        // --- Groups: ensure state + uniforms ---
        // Non-passthrough groups need the full group_state + blend uniforms.
        // Passthrough groups may still own a `passthrough_mask_state` whose
        // `isolated` lerp uniform must track the engine's isolation target,
        // so we update them through the same path — `update_group_uniforms`
        // skips the group_state branch when none exists and writes only the
        // pms uniform.
        let groups: Vec<(LayerId, bool, f32, u32, bool)> = self
            .doc
            .all_groups()
            .iter()
            .map(|g| {
                (
                    g.id,
                    g.passthrough,
                    g.blend.opacity,
                    g.blend.blend_mode.gpu_value,
                    isolated_host(g.id),
                )
            })
            .collect();
        for (id, passthrough, opacity, blend_mode_gpu, isolated_flag) in groups {
            if !passthrough {
                self.compositor
                    .ensure_group_state(&self.gpu.device, &self.gpu.queue, id);
            }
            self.compositor.update_group_uniforms(
                &self.gpu.queue,
                id,
                opacity,
                blend_mode_gpu,
                isolated_flag,
            );
        }
    }
}

enum UndoDirection {
    Undo,
    Redo,
}

/// Encode a region restore into `frame` and swap the produced redo-side
/// entry back into `*entry`. Shared by the layer-pixels and selection
/// branches of `apply_undo` — only the frame source (node texture vs
/// selection state) and the post-restore side effects differ between
/// callers. Kept as a free function so the caller can hold a
/// `CanvasFrame<'_>` borrowed from `self.compositor` while passing
/// `&mut self.region_store` — field-level borrow splitting that a
/// `&mut self` method couldn't express.
fn restore_gpu_region(
    gpu: &GpuContext,
    region_store: &mut RegionStore,
    entry: &mut UndoRegionEntry,
    frame: &CanvasFrame<'_>,
    label: &str,
) {
    gpu.encode(label, |encoder| {
        let swapped = region_store.restore_region(encoder, entry, frame);
        *entry = swapped;
    });
}

// ---------------------------------------------------------------------------
// Thumbnail generation — nearest-neighbor sampling from GPU readback pixels
// ---------------------------------------------------------------------------

fn generate_rgba_thumbnail_from_pixels(
    pixels: &[u8],
    doc_w: u32,
    doc_h: u32,
    thumb_w: u32,
    thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);

            let src = ((cy * doc_w + cx) * 4) as usize;
            let (r, g, b, a) = if src + 3 < pixels.len() {
                (
                    pixels[src],
                    pixels[src + 1],
                    pixels[src + 2],
                    pixels[src + 3],
                )
            } else {
                (0, 0, 0, 0)
            };

            let off = ((oy * thumb_w + ox) * 4) as usize;
            // Checkerboard behind transparent areas
            let check = if ((ox / 4) + (oy / 4)) % 2 == 0 {
                102u8
            } else {
                153u8
            };
            let af = a as f32 / 255.0;
            buf[off] = (r as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 1] = (g as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 2] = (b as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 3] = 255;
        }
    }
    buf
}

/// Output side length for cached dab thumbnails. The bake renders into
/// a larger canvas (see `BRUSH_DAB_RENDER_SIZE`) so brushes whose dabs
/// are tiny or off-center have enough headroom; the framer below crops
/// to the actual content and downscales here so picker tiles always
/// see a stably-sized PNG regardless of how the brush graph chose to
/// place its stamp.
const DAB_THUMBNAIL_OUTPUT_SIZE: u32 = 96;

/// Frame a rendered dab into a centered, content-fitted PNG.
///
/// Generic across every brush — no per-brush logic. The procedure:
///   1. Scan for non-bg pixels (anything outside the theme bg by more
///      than a small tolerance) and compute their bounding box.
///   2. Square the bbox (use the longer side), inflate by 10% margin,
///      and re-center on the bbox centroid, clamped to canvas bounds.
///   3. Resize the cropped square to `DAB_THUMBNAIL_OUTPUT_SIZE`.
///
/// Brushes that already fill the canvas bbox to the
/// canvas → just downscaled. Brushes that paint a small dot (Airbrush)
/// bbox to the dot → upscaled into the frame. Brushes that displace
/// the dab off-center (Scatter Brush) bbox to wherever the displaced
/// dab landed → crop re-centers it. Empty renders (degenerate brushes,
/// or a scatter that hit fully off-canvas) fall through to a centered
/// square of the bg, which the picker shows as a flat tile.
fn frame_dab_thumbnail(pixels: &[u8], width: u32, height: u32, bg: [f32; 4]) -> Vec<u8> {
    let expected = (width * height * 4) as usize;
    if pixels.len() < expected {
        log::error!(
            "dab thumbnail pixel buffer too small: {} < {expected}",
            pixels.len()
        );
        return Vec::new();
    }
    let bg_u8 = [
        (bg[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (bg[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (bg[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    ];
    // Tolerance accommodates the GPU's premultiplied-alpha rounding
    // and any color-management drift; tight enough to still pick up a
    // pale stroke against the bg.
    const TOLERANCE: i32 = 12;

    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let dr = (pixels[i] as i32 - bg_u8[0] as i32).abs();
            let dg = (pixels[i + 1] as i32 - bg_u8[1] as i32).abs();
            let db = (pixels[i + 2] as i32 - bg_u8[2] as i32).abs();
            if dr > TOLERANCE || dg > TOLERANCE || db > TOLERANCE {
                if x < min_x {
                    min_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if x > max_x {
                    max_x = x;
                }
                if y > max_y {
                    max_y = y;
                }
                found = true;
            }
        }
    }

    let Some(src) = image::RgbaImage::from_raw(width, height, pixels.to_vec()) else {
        return Vec::new();
    };

    let cropped = if found {
        let bbox_w = max_x - min_x + 1;
        let bbox_h = max_y - min_y + 1;
        let raw_side = bbox_w.max(bbox_h);
        let margin = (raw_side / 10).max(2);
        // Square crop, clamped to the smaller canvas dim.
        let side = (raw_side + 2 * margin).min(width.min(height));
        let cx = min_x + bbox_w / 2;
        let cy = min_y + bbox_h / 2;
        let half = side / 2;
        // `half` may exceed the centroid → saturating_sub clamps to 0;
        // the upper clamp keeps the crop fully inside the canvas.
        let crop_x = cx.saturating_sub(half).min(width - side);
        let crop_y = cy.saturating_sub(half).min(height - side);
        image::imageops::crop_imm(&src, crop_x, crop_y, side, side).to_image()
    } else {
        // Empty render — centered square of bg. Visible as a flat tile.
        let side = width.min(height);
        let crop_x = (width - side) / 2;
        let crop_y = (height - side) / 2;
        image::imageops::crop_imm(&src, crop_x, crop_y, side, side).to_image()
    };

    let resized = image::imageops::resize(
        &cropped,
        DAB_THUMBNAIL_OUTPUT_SIZE,
        DAB_THUMBNAIL_OUTPUT_SIZE,
        image::imageops::FilterType::Triangle,
    );

    let mut out = Vec::new();
    let cursor = std::io::Cursor::new(&mut out);
    let encoder = image::codecs::png::PngEncoder::new(cursor);
    use image::ImageEncoder;
    if let Err(e) = encoder.write_image(
        resized.as_raw(),
        DAB_THUMBNAIL_OUTPUT_SIZE,
        DAB_THUMBNAIL_OUTPUT_SIZE,
        image::ExtendedColorType::Rgba8,
    ) {
        log::error!("dab thumbnail PNG encode failed: {e}");
        return Vec::new();
    }
    out
}

/// Frame a rendered stroke into the cache aspect ratio and resize.
///
/// Same shape as `frame_dab_thumbnail` but for the S-curve preview:
///   1. Scan for non-bg pixels and compute their bounding box.
///   2. Expand the bbox to match the target aspect ratio so the stroke
///      isn't squashed by the resize.
///   3. Inflate by a 10% margin on each axis, then re-center on the
///      bbox centroid and clamp to the source bounds.
///   4. Resize the cropped region to `(dst_w, dst_h)`.
///
/// Brush size doesn't enter into any of this — bigger dabs paint a
/// bigger bbox, smaller dabs paint a smaller bbox, the framer fits
/// either to the target. The preview path is the same for every brush.
fn frame_stroke_thumbnail(
    pixels: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    bg: [f32; 4],
) -> Vec<u8> {
    let expected = (src_w * src_h * 4) as usize;
    if pixels.len() < expected || dst_w == 0 || dst_h == 0 {
        log::error!(
            "stroke thumbnail pixel buffer too small: {} < {expected}",
            pixels.len()
        );
        return Vec::new();
    }
    let bg_u8 = [
        (bg[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (bg[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (bg[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    ];
    // Same tolerance shape as frame_dab_thumbnail — accommodates
    // premultiplied-alpha rounding on the GPU side.
    const TOLERANCE: i32 = 12;

    let mut min_x = src_w;
    let mut min_y = src_h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;
    for y in 0..src_h {
        for x in 0..src_w {
            let i = ((y * src_w + x) * 4) as usize;
            let dr = (pixels[i] as i32 - bg_u8[0] as i32).abs();
            let dg = (pixels[i + 1] as i32 - bg_u8[1] as i32).abs();
            let db = (pixels[i + 2] as i32 - bg_u8[2] as i32).abs();
            if dr > TOLERANCE || dg > TOLERANCE || db > TOLERANCE {
                if x < min_x {
                    min_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if x > max_x {
                    max_x = x;
                }
                if y > max_y {
                    max_y = y;
                }
                found = true;
            }
        }
    }

    let Some(src) = image::RgbaImage::from_raw(src_w, src_h, pixels.to_vec()) else {
        return Vec::new();
    };

    let cropped = if found {
        let bbox_w = max_x - min_x + 1;
        let bbox_h = max_y - min_y + 1;
        let target_aspect = dst_w as f32 / dst_h as f32;
        let bbox_aspect = bbox_w as f32 / bbox_h as f32;

        // Aspect-fit: expand whichever axis is short of the target
        // aspect so the resize doesn't squash the stroke.
        let (mut crop_w, mut crop_h) = if bbox_aspect < target_aspect {
            let w = (bbox_h as f32 * target_aspect).ceil() as u32;
            (w.max(bbox_w), bbox_h)
        } else {
            let h = (bbox_w as f32 / target_aspect).ceil() as u32;
            (bbox_w, h.max(bbox_h))
        };

        // 10% margin on each axis, floor 2 px (matches frame_dab_thumbnail).
        let margin_w = (crop_w / 10).max(2);
        let margin_h = (crop_h / 10).max(2);
        crop_w = (crop_w + 2 * margin_w).min(src_w);
        crop_h = (crop_h + 2 * margin_h).min(src_h);

        let cx = min_x + bbox_w / 2;
        let cy = min_y + bbox_h / 2;
        let crop_x = cx.saturating_sub(crop_w / 2).min(src_w - crop_w);
        let crop_y = cy.saturating_sub(crop_h / 2).min(src_h - crop_h);
        image::imageops::crop_imm(&src, crop_x, crop_y, crop_w, crop_h).to_image()
    } else {
        // Empty render — return a flat field of bg at the target size.
        // Skip the resize entirely; constructing it directly is cheaper
        // and avoids the resize filter introducing rounding.
        let mut buf = Vec::with_capacity((dst_w * dst_h * 4) as usize);
        let bg_a = (bg[3].clamp(0.0, 1.0) * 255.0).round() as u8;
        for _ in 0..(dst_w * dst_h) {
            buf.extend_from_slice(&[bg_u8[0], bg_u8[1], bg_u8[2], bg_a]);
        }
        return buf;
    };

    let resized = image::imageops::resize(
        &cropped,
        dst_w,
        dst_h,
        image::imageops::FilterType::Triangle,
    );

    resized.into_raw()
}

/// Encode an RGBA8 buffer as a PNG. Used for baking brush thumbnails —
/// the PNG goes into the `.darkly-brush` ZIP as `preview.png`.
fn encode_rgba_as_png(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let expected = (width * height * 4) as usize;
    if pixels.len() < expected {
        log::error!(
            "brush thumbnail pixel buffer too small: {} < {expected}",
            pixels.len()
        );
        return Vec::new();
    }
    let mut out = Vec::with_capacity(expected / 4);
    let cursor = std::io::Cursor::new(&mut out);
    let encoder = image::codecs::png::PngEncoder::new(cursor);
    use image::ImageEncoder;
    if let Err(e) = encoder.write_image(
        &pixels[..expected],
        width,
        height,
        image::ExtendedColorType::Rgba8,
    ) {
        log::error!("brush thumbnail PNG encode failed: {e}");
        return Vec::new();
    }
    out
}

fn generate_mask_thumbnail_from_pixels(
    pixels: &[u8],
    doc_w: u32,
    doc_h: u32,
    thumb_w: u32,
    thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);

            let v = pixels
                .get((cy * doc_w + cx) as usize)
                .copied()
                .unwrap_or(255);

            let off = ((oy * thumb_w + ox) * 4) as usize;
            buf[off] = v;
            buf[off + 1] = v;
            buf[off + 2] = v;
            buf[off + 3] = 255;
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `src_w * src_h` RGBA buffer filled with `bg`, then paint a
    /// solid rectangle of `fg` at `(x0..x1, y0..y1)`.
    fn fill_with_rect(
        src_w: u32,
        src_h: u32,
        bg: [u8; 4],
        fg: [u8; 4],
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; (src_w * src_h * 4) as usize];
        for y in 0..src_h {
            for x in 0..src_w {
                let i = ((y * src_w + x) * 4) as usize;
                let c = if x >= x0 && x < x1 && y >= y0 && y < y1 {
                    fg
                } else {
                    bg
                };
                buf[i..i + 4].copy_from_slice(&c);
            }
        }
        buf
    }

    #[test]
    fn frame_stroke_empty_render_returns_bg_field() {
        let bg = [0.05, 0.05, 0.05, 1.0];
        let bg_u8 = [13u8, 13, 13, 255];
        let pixels = fill_with_rect(640, 240, bg_u8, bg_u8, 0, 0, 0, 0);
        let framed = frame_stroke_thumbnail(&pixels, 640, 240, 320, 120, bg);
        assert_eq!(framed.len(), (320 * 120 * 4) as usize);
        // Every pixel matches bg.
        for chunk in framed.chunks_exact(4) {
            assert_eq!(chunk[0], bg_u8[0]);
            assert_eq!(chunk[1], bg_u8[1]);
            assert_eq!(chunk[2], bg_u8[2]);
        }
    }

    #[test]
    fn frame_stroke_tiny_bbox_is_upscaled() {
        // Small white square in the middle of a 640x240 dark canvas.
        // After framing, the central region should be majority bright.
        let bg = [0.0, 0.0, 0.0, 1.0];
        let pixels = fill_with_rect(
            640,
            240,
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            315,
            115,
            325,
            125,
        );
        let framed = frame_stroke_thumbnail(&pixels, 640, 240, 320, 120, bg);
        assert_eq!(framed.len(), (320 * 120 * 4) as usize);
        // Center 80x40 region should be predominantly bright.
        let mut bright = 0;
        for y in 40..80 {
            for x in 120..200 {
                let i = ((y * 320 + x) * 4) as usize;
                if framed[i] > 128 {
                    bright += 1;
                }
            }
        }
        assert!(
            bright > 1000,
            "expected upscaled square to fill most of center region, got {bright}"
        );
    }

    #[test]
    fn frame_stroke_fullcanvas_bbox_resizes_down() {
        // White stripe across the full width; bbox spans the whole canvas.
        // Output should still contain the stripe (resized from 640x240 to
        // 320x120) and not collapse to bg.
        let bg = [0.0, 0.0, 0.0, 1.0];
        let pixels = fill_with_rect(
            640,
            240,
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            0,
            110,
            640,
            130,
        );
        let framed = frame_stroke_thumbnail(&pixels, 640, 240, 320, 120, bg);
        let bright = framed.chunks_exact(4).filter(|p| p[0] > 128).count();
        assert!(
            bright > 100,
            "full-canvas stripe should survive the downscale, got {bright} bright pixels"
        );
    }

    #[test]
    fn frame_stroke_off_center_bbox_is_recentered() {
        // Stripe in the upper-left quadrant only. The framer should crop
        // around it so the stripe is visible somewhere in the framed
        // output (not just at the upper-left).
        let bg = [0.0, 0.0, 0.0, 1.0];
        let pixels = fill_with_rect(
            640,
            240,
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            10,
            10,
            120,
            30,
        );
        let framed = frame_stroke_thumbnail(&pixels, 640, 240, 320, 120, bg);
        let bright = framed.chunks_exact(4).filter(|p| p[0] > 128).count();
        assert!(
            bright > 200,
            "off-center stripe should appear in framed output, got {bright}"
        );
    }
}
