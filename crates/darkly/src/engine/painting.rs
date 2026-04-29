//! Stroke lifecycle, flood fill, erase helpers, and paint infrastructure.

use super::types::StrokeOp;
use super::{DarklyEngine, PendingUndoCommit, ReadbackContext};
use crate::brush::checkpoint_ring::CheckpointRing;
use crate::brush::gpu_context::BrushGpuContext;
use crate::brush::paint_info::PaintInformation;
use crate::brush::spacing::SpacingConfig;
use crate::brush::stroke_buffer::StrokeBuffer;
use crate::brush::stroke_engine::StrokeEngine;
use crate::gpu::flood_fill;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback;
use crate::undo::GpuRegionAction;

impl DarklyEngine {
    /// Read the stabilize strength from the pen_input node's "stabilize" port
    /// default in the active brush graph.  Returns 0.0 if not found.
    fn pen_input_stabilize_strength(&self) -> f32 {
        use crate::nodegraph::PortDir;
        for node in self.active_brush_graph.nodes.values() {
            if node.type_id == "pen_input" {
                for port in &node.ports {
                    if port.name == "stabilize" && port.dir == PortDir::Input {
                        return port.default;
                    }
                }
            }
        }
        0.0
    }

    /// Read the dab spacing ratio from the pen_input node's "spacing" port
    /// default. Falls back to `SpacingConfig::default().ratio` for graphs
    /// that predate the port (loaded from older brushes).
    fn pen_input_spacing_ratio(&self) -> f32 {
        use crate::nodegraph::PortDir;
        for node in self.active_brush_graph.nodes.values() {
            if node.type_id == "pen_input" {
                for port in &node.ports {
                    if port.name == "spacing" && port.dir == PortDir::Input {
                        return port.default;
                    }
                }
            }
        }
        SpacingConfig::default().ratio
    }

    /// Flush any pending diff-based undo commit. Called before overwriting the
    /// scratch texture (e.g. at the start of a new stroke). Uses Poll (not Wait)
    /// — if the diff hasn't completed yet, falls back to a full-canvas rect.
    pub(crate) fn flush_pending_undo_commit(&mut self) {
        if !self.diff_rect.is_pending() {
            return;
        }
        let Some(commit) = self.pending_undo_commit.take() else {
            return;
        };

        // Try to collect the result without blocking.
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);
        let rect = match self.diff_rect.poll(&self.gpu.device) {
            Some(Some(rect)) => rect,
            Some(None) => return, // Textures identical — no commit needed.
            None => {
                // Diff not ready — fall back to full canvas.
                let (w, h) = self.region_store.scratch_dimensions();
                [0, 0, w, h]
            }
        };

        self.gpu.encode("brush-stroke-end-flush", |encoder| {
            let entry =
                self.region_store
                    .commit_region(encoder, commit.layer_id, commit.format, rect);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
    }

    // --- Painting ---

    /// Fill the layer with the default background image, centered and clipped
    /// to the canvas. The image is baked into the binary at build time.
    pub fn fill_background(&mut self, layer_id: u64) {
        const IMAGE_BYTES: &[u8] = include_bytes!("../../resources/backgrounds/quiet-night.jpg");

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let rect = [0, 0, canvas_w, canvas_h];
        let format = wgpu::TextureFormat::Rgba8Unorm;

        let layer_tex = match self.compositor.layer_texture(layer_id) {
            Some(t) => t,
            None => return,
        };

        // Save current state to scratch for undo.
        self.gpu.encode("fill-background-save", |encoder| {
            self.region_store
                .save_region(encoder, &layer_tex.texture, format, rect);
            let entry = self
                .region_store
                .commit_region(encoder, layer_id, format, rect);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });

        let decoded = image::load_from_memory(IMAGE_BYTES)
            .expect("failed to decode embedded background image")
            .to_rgba8();
        let (img_w, img_h) = decoded.dimensions();

        // Center the image on the canvas, clipped to canvas bounds.
        let offset_x = (canvas_w as i32 - img_w as i32) / 2;
        let offset_y = (canvas_h as i32 - img_h as i32) / 2;
        let src_x = (-offset_x).max(0) as u32;
        let src_y = (-offset_y).max(0) as u32;
        let dst_x = offset_x.max(0) as u32;
        let dst_y = offset_y.max(0) as u32;
        let copy_w = (img_w - src_x).min(canvas_w - dst_x);
        let copy_h = (img_h - src_y).min(canvas_h - dst_y);

        if copy_w > 0 && copy_h > 0 {
            let layer_tex = self.compositor.layer_texture(layer_id).unwrap();
            let row_bytes = copy_w as usize * 4;
            let mut buf = vec![0u8; row_bytes * copy_h as usize];
            let full = decoded.as_raw();
            for row in 0..copy_h as usize {
                let src_row = (src_y as usize + row) * img_w as usize * 4 + src_x as usize * 4;
                let dst_row = row * row_bytes;
                buf[dst_row..dst_row + row_bytes]
                    .copy_from_slice(&full[src_row..src_row + row_bytes]);
            }
            self.gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &layer_tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_x,
                        y: dst_y,
                        z: 0,
                    },
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

        self.compositor.mark_dirty();
    }

    // --- Stroke lifecycle ---
    // Following GIMP's edit_mask flag: when editing_mask_layer is set,
    // strokes are routed to the mask instead of the layer.
    //
    // All stroke ops go through GPU render passes (Phase 3).

    pub fn begin_stroke(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        self.doc
            .set_mask_editing(if self.editing_mask_layer == Some(layer_id) {
                Some(layer_id)
            } else {
                None
            });
        self.active_stroke_layer = Some(layer_id);
        // GPU setup is deferred to first stroke_to (lazy init).
    }

    pub fn stroke_to(&mut self, op: StrokeOp) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };
        self.gpu_stroke_to(layer_id, op);
    }

    /// GPU paint path for all stroke operations.
    fn gpu_stroke_to(&mut self, layer_id: u64, op: StrokeOp) {
        let mask_editing = self.editing_mask_layer == Some(layer_id);
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        // Brush strokes may extend past the layer's current canvas extent
        // (e.g. paste-extent layers, or any stroke that wanders past the
        // canvas). Grow the layer texture in chunked steps so the dab
        // dispatch and undo paths see a sufficiently-large layer.
        // Non-BrushStroke ops (gradient, flood fill, fill rect) operate on
        // existing pixels and don't need preemptive growth.
        if let StrokeOp::BrushStroke { x, y, .. } = op {
            self.ensure_layer_covers_dab(layer_id, mask_editing, x, y);
        }

        // Lazy init: save the layer to scratch for undo on first stroke_to.
        // Use the layer's actual texture dimensions (not canvas) so paste-
        // extent layers preserve off-canvas pixels through undo.
        if !self.scratch_saved {
            self.flush_pending_undo_commit();
            let (texture, format, layer_w, layer_h) = if mask_editing {
                match self.compositor.mask_texture(layer_id) {
                    Some(t) => (&t.texture, wgpu::TextureFormat::R8Unorm, t.width, t.height),
                    None => return,
                }
            } else {
                match self.compositor.layer_texture(layer_id) {
                    Some(t) => (
                        &t.texture,
                        wgpu::TextureFormat::Rgba8Unorm,
                        t.width,
                        t.height,
                    ),
                    None => return,
                }
            };

            self.region_store
                .ensure_scratch_capacity(&self.gpu.device, layer_w, layer_h);
            self.gpu.encode("stroke-begin", |encoder| {
                self.region_store
                    .save_region(encoder, texture, format, [0, 0, layer_w, layer_h]);
            });

            self.scratch_saved = true;
        }

        macro_rules! paint_target {
            () => {
                if mask_editing {
                    self.compositor
                        .mask_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_mask(t, canvas_w, canvas_h))
                } else {
                    self.compositor
                        .layer_texture(layer_id)
                        .map(|t| GpuPaintTarget::from_layer(t, canvas_w, canvas_h))
                }
            };
        }

        match op {
            StrokeOp::LinearGradient {
                x0,
                y0,
                x1,
                y1,
                r0,
                g0,
                b0,
                a0,
                r1,
                g1,
                b1,
                a1,
            } => {
                let target = match paint_target!() {
                    Some(t) => t,
                    None => return,
                };
                self.gpu.encode("stroke-gradient", |encoder| {
                    target.linear_gradient(
                        encoder,
                        &self.paint_pipelines,
                        &self.gpu.queue,
                        x0,
                        y0,
                        x1,
                        y1,
                        [r0, g0, b0, a0],
                        [r1, g1, b1, a1],
                        None,
                    );
                });
            }
            StrokeOp::FloodFill {
                x,
                y,
                r,
                g,
                b,
                a,
                tolerance,
            } => {
                self.gpu_flood_fill(
                    layer_id,
                    mask_editing,
                    x as i32,
                    y as i32,
                    [r, g, b, a],
                    tolerance,
                    canvas_w,
                    canvas_h,
                );
            }
            StrokeOp::BrushStroke {
                x,
                y,
                pressure,
                x_tilt,
                y_tilt,
                rotation,
                tangential_pressure,
                time_ms,
                cr,
                cg,
                cb,
                ca,
            } => {
                self.brush_stroke_to(
                    layer_id,
                    mask_editing,
                    x,
                    y,
                    pressure,
                    x_tilt,
                    y_tilt,
                    rotation,
                    tangential_pressure,
                    time_ms,
                    [cr, cg, cb, ca],
                    canvas_w,
                    canvas_h,
                );
            }
        }

        self.compositor.mark_dirty();
    }

    /// Grow the layer texture if the next dab at canvas `(x, y)` would land
    /// outside its current bounds. Triggered only when the dab CENTER falls
    /// outside the layer's canvas extent — strokes within the layer don't
    /// extend it (matching Krita's behavior of growing only when paint
    /// actually escapes the layer's recorded bounds; dab footprints that
    /// cross a layer edge are GPU-clipped).
    ///
    /// On growth, the StrokeBuffer scratch and RegionStore scratch are both
    /// re-anchored to the new layer's local coordinate system so canvas-
    /// space pre-stroke pixels remain in the right place; bind groups
    /// referencing the old textures are rebuilt by their owners. Layer
    /// blend uniforms are refreshed so the next composite pass sees the
    /// new offset/size.
    ///
    /// The `needed` rect padded outward by `MAX_DAB_SIZE/2` so the new
    /// chunk-aligned extent comfortably covers the dab's worst-case
    /// footprint, not just its center pixel.
    fn ensure_layer_covers_dab(&mut self, layer_id: u64, mask_editing: bool, x: f32, y: f32) {
        // Fetch the current layer extent before mutating the compositor.
        let current_extent = if mask_editing {
            match self.compositor.mask_texture(layer_id) {
                Some(t) => t.canvas_extent(),
                None => return,
            }
        } else {
            match self.compositor.layer_texture(layer_id) {
                Some(t) => t.canvas_extent(),
                None => return,
            }
        };

        // Trigger: dab center outside current extent. Doesn't grow when the
        // user paints inside the canvas with a brush whose footprint
        // happens to cross the canvas edge — those edge pixels would clip
        // anyway with the canvas-aligned layer, matching pre-P2 behavior.
        let cx = x.floor() as i32;
        let cy = y.floor() as i32;
        if cx >= current_extent.x0()
            && cx < current_extent.x1()
            && cy >= current_extent.y0()
            && cy < current_extent.y1()
        {
            return;
        }

        // Center-out-of-bounds: pad the requested rect by half of
        // MAX_DAB_SIZE so the grown extent includes the dab's footprint.
        const HALF: i32 = (crate::brush::dab_pool::MAX_DAB_SIZE / 2) as i32;
        let needed = crate::coord::CanvasRect::from_xywh(
            cx - HALF,
            cy - HALF,
            (HALF as u32) * 2,
            (HALF as u32) * 2,
        );

        // Encoder discipline: the grow + scratch rebase must run in their
        // own encoder, submitted before any subsequent dab dispatch can
        // start a new encoder against the new texture. `gpu.encode` already
        // does one-encoder-per-call.
        let outcome = self.gpu.encode_ret("layer-grow", |encoder| {
            self.compositor.grow_layer_texture(
                &self.gpu.device,
                &self.gpu.queue,
                encoder,
                layer_id,
                needed,
            )
        });

        let new_extent = match outcome {
            crate::gpu::compositor::GrowOutcome::Grown { new_extent } => new_extent,
            crate::gpu::compositor::GrowOutcome::NoChange => return,
            crate::gpu::compositor::GrowOutcome::AtCap => return,
        };

        let dx = (current_extent.origin.x - new_extent.origin.x) as u32;
        let dy = (current_extent.origin.y - new_extent.origin.y) as u32;

        // Re-anchor the StrokeBuffer scratch + pre-stroke snapshot. The
        // bind groups inside the StrokeBuffer reference the old textures
        // and are rebuilt against the new ones.
        if let Some(stroke_buffer) = self.stroke_buffer.as_mut() {
            self.gpu.encode("stroke-buffer-grow", |encoder| {
                stroke_buffer.grow_preserving(
                    &self.gpu.device,
                    encoder,
                    new_extent.width,
                    new_extent.height,
                    dx,
                    dy,
                    self.dab_pool.bind_group_layout(),
                    self.brush_pipelines.canvas_copy_bind_group_layout(),
                );
            });
        }

        // Re-anchor the region_store scratch so the diff_rect at
        // end_stroke compares matching coordinate frames. If the scratch
        // hasn't been saved yet (this is the first dab and lazy init
        // hasn't run), the rebase is a no-op on still-empty contents.
        if self.scratch_saved {
            self.gpu.encode("region-scratch-grow", |encoder| {
                self.region_store.grow_scratch_preserving(
                    &self.gpu.device,
                    encoder,
                    new_extent.width,
                    new_extent.height,
                    dx,
                    dy,
                );
            });
        } else {
            // Lazy init will allocate the scratch at the new dimensions
            // when it next saves; just bump capacity now so the save
            // doesn't trigger another reallocation.
            self.region_store.ensure_scratch_capacity(
                &self.gpu.device,
                new_extent.width,
                new_extent.height,
            );
        }

        // Update the document's authoritative bounds and refresh the
        // layer's blend uniforms so the composite pass sees the new
        // offset/size on the next render.
        let bounds = crate::layer::LayerBounds {
            offset_x: new_extent.origin.x,
            offset_y: new_extent.origin.y,
            width: new_extent.width,
            height: new_extent.height,
        };
        if let Some(crate::layer::Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
            r.bounds = bounds;
            let opacity = r.opacity;
            let blend_mode = r.blend_mode;
            let show_mask = r.show_mask;
            self.compositor.update_raster_uniforms_full(
                &self.gpu.queue,
                layer_id,
                opacity,
                blend_mode,
                show_mask,
            );
        }
    }

    /// Handle a BrushStroke event through the node-graph brush engine.
    ///
    /// Lazy-inits a `StrokeEngine` + `StrokeBuffer` on the first event.
    /// Each event feeds through the stabilizer, which may trigger rewind
    /// and re-rendering of the stroke from scratch.
    fn brush_stroke_to(
        &mut self,
        layer_id: u64,
        mask_editing: bool,
        x: f32,
        y: f32,
        pressure: f32,
        x_tilt: f32,
        y_tilt: f32,
        rotation: f32,
        tangential_pressure: f32,
        time_ms: f64,
        color: [f32; 4],
        canvas_w: u32,
        canvas_h: u32,
    ) {
        // True on the lazy-init path below — the terminal's `begin_stroke`
        // hook must run once before the first dab to initialise the scratch.
        let mut need_begin_stroke = false;

        // Lazy-init: compile the active brush graph + create stroke buffer.
        if self.brush_stroke_engine.is_none() {
            need_begin_stroke = true;
            let runner = match crate::brush::compile_graph(&self.active_brush_graph) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("brush graph compilation failed: {e:?}");
                    return;
                }
            };

            // Derive stabilizer from the pen_input node's "stabilize" port.
            let strength = self.pen_input_stabilize_strength();
            let stabilizer_config = if strength > 0.0 {
                crate::brush::stabilizer::StabilizerConfig {
                    algorithm: "laplacian".into(),
                    params: vec![crate::gpu::params::ParamValue::Float(strength)],
                }
            } else {
                crate::brush::stabilizer::StabilizerConfig::default()
            };
            let stabilizer = self
                .stabilizer_registry
                .create_from_config(&stabilizer_config);

            self.brush_stroke_engine = Some(StrokeEngine::new(
                runner,
                color,
                SpacingConfig {
                    ratio: self.pen_input_spacing_ratio(),
                    ..SpacingConfig::default()
                },
                stabilizer,
            ));

            // Create the stroke buffer and save the pre-stroke snapshot.
            let layer_tex = if mask_editing {
                self.compositor.mask_texture(layer_id)
            } else {
                self.compositor.layer_texture(layer_id)
            };
            if let Some(layer_tex) = layer_tex {
                // Size the stroke scratch and pre-stroke snapshot to the
                // layer's bounds. For paste-extent layers larger than the
                // canvas this means dabs landing on off-canvas pixels are
                // saved/restored correctly on undo.
                let stroke_buffer = StrokeBuffer::new(
                    &self.gpu.device,
                    layer_tex.width,
                    layer_tex.height,
                    self.dab_pool.bind_group_layout(),
                    self.brush_pipelines.canvas_copy_bind_group_layout(),
                );
                self.gpu.encode("stroke-buffer-init", |encoder| {
                    stroke_buffer.save_pre_stroke(encoder, &layer_tex.texture);
                });
                // Scratch initialisation is now the terminal's responsibility
                // (via `runner.begin_stroke`). Deferred until we have the
                // engine + buffer in hand a few lines below — see the
                // `begin_stroke` call guarded by `first_event`.
                self.stroke_buffer = Some(stroke_buffer);
            }
        }

        // Build PaintInformation from the raw tablet data.
        let info = PaintInformation {
            pos: [x, y],
            pressure,
            x_tilt,
            y_tilt,
            rotation,
            tangential_pressure,
            time: (time_ms / 1000.0) as f32,
            ..Default::default()
        };

        // Get the canvas texture and view.
        let layer_tex = if mask_editing {
            match self.compositor.mask_texture(layer_id) {
                Some(t) => t,
                None => return,
            }
        } else {
            match self.compositor.layer_texture(layer_id) {
                Some(t) => t,
                None => return,
            }
        };
        let layer_view = layer_tex.view.clone();

        // Take the stroke engine and buffer out to avoid borrow conflicts.
        let mut engine = self.brush_stroke_engine.take().unwrap();
        let stroke_buffer = self.stroke_buffer.take();

        let sel_bg = if self.gpu_selection.active {
            self.gpu_selection.brush_bind_group()
        } else {
            &self.brush_pipelines.default_selection_bind_group
        };

        if let Some(ref stroke_buffer) = stroke_buffer {
            // Stabilized path: dabs render into the scratch, then the
            // terminal's `commit` hook lands them on the layer.
            self.brush_pipelines.reset_uniform_rings();
            let result = engine.stabilize(info);
            let max_div = engine.max_divergence_window();
            let tip_vi = engine.stabilizer_len().saturating_sub(1);

            // Synthesize divergence on the previously-rendered tip segment.
            // It was drawn with a degenerate `p3 = p2` because the next
            // sample hadn't arrived yet; now it has, so re-render that
            // segment with proper Catmull-Rom lookahead.  `tip_div` is
            // the deeper of the two when the stabilizer also reports
            // divergence (take the earliest vi that needs rebuild).
            let tip_div = tip_vi.saturating_sub(1);
            let div_idx = match result.divergence_index {
                Some(k) => Some(k.min(tip_div)),
                None if tip_vi >= 1 => Some(tip_div),
                None => None,
            };

            // Helper macro: create a BrushGpuContext wired with the stroke
            // scratch, layer, and pre-stroke snapshot. The macro always
            // includes the commit-side references (layer_view, bind groups,
            // pre_stroke_texture) — `color_output::commit` asks for them,
            // and lifecycle hooks fan out from the same context.
            macro_rules! make_gpu_ctx {
                ($label:expr) => {
                    BrushGpuContext {
                        encoder: self.gpu.device.create_command_encoder(
                            &wgpu::CommandEncoderDescriptor {
                                label: Some($label),
                            },
                        ),
                        device: &self.gpu.device,
                        queue: &self.gpu.queue,
                        dab_pool: &mut self.dab_pool,
                        pipelines: &self.brush_pipelines,
                        stroke_scratch_view: stroke_buffer.stroke_view(),
                        stroke_scratch_texture: stroke_buffer.stroke_texture(),
                        canvas_width: canvas_w,
                        canvas_height: canvas_h,
                        layer_width: layer_tex.width,
                        layer_height: layer_tex.height,
                        layer_offset_x: layer_tex.offset_x,
                        layer_offset_y: layer_tex.offset_y,
                        selection_bind_group: sel_bg,
                        resource_handles: &self.resource_handles,
                        // blend_mode applies at commit (paint vs. erase). The
                        // per-dab composite inside `color_output::evaluate_gpu`
                        // hard-codes source-over regardless of this value.
                        blend_mode: self.brush_blend_mode,
                        canvas_copy_origin: None,
                        preview_mask_view: None,
                        preview_mask_size: (0, 0),
                        brush_preview_info: None,
                        layer_view: Some(&layer_view),
                        layer_texture: Some(&layer_tex.texture),
                        pre_stroke_texture: Some(stroke_buffer.pre_stroke_texture()),
                        pre_stroke_bind_group: Some(stroke_buffer.pre_stroke_bind_group()),
                        scratch_bind_group: Some(stroke_buffer.stroke_bind_group()),
                        dab_write_bbox: None,
                    }
                };
            }

            // First event of the stroke — let the terminal set up its scratch.
            if need_begin_stroke {
                let mut gpu_ctx = make_gpu_ctx!("brush-begin-stroke");
                engine.begin_stroke(&mut gpu_ctx);
                gpu_ctx.submit_final();
            }

            if let Some(div_idx) = div_idx {
                // Divergence — try checkpoint-based partial re-render.
                // The terminal's `begin_stroke` establishes outside-bbox
                // state for whichever path we take below; the checkpoint
                // ring no longer clears on its own.
                {
                    let mut gpu_ctx = make_gpu_ctx!("brush-begin-stroke-rewind");
                    engine.begin_stroke(&mut gpu_ctx);
                    gpu_ctx.submit_final();
                }

                let restore = self.gpu.encode_ret("stroke-checkpoint-restore", |encoder| {
                    self.checkpoint_ring.restore_before(
                        encoder,
                        stroke_buffer.stroke_texture(),
                        div_idx,
                    )
                });

                let start_vi = if let Some(cp) = restore {
                    // Restored from checkpoint — truncate and resume.
                    engine.save_points.truncate(cp.save_point_index + 1);
                    engine.restore_render_state(&cp.render_state);
                    // Only invalidate from the divergence point onward —
                    // checkpoints between the restore point and div_idx
                    // are still valid (the stroke buffer content there
                    // didn't change, only positions >= div_idx diverged).
                    self.checkpoint_ring.invalidate_from(div_idx);
                    cp.vector_index + 1
                } else {
                    // No checkpoint before divergence — full re-render. The
                    // `begin_stroke` above already reset the scratch.
                    engine.reset_render_state();
                    self.checkpoint_ring.clear();
                    0
                };
                // Render in segments with checkpoints at boundaries.
                let boundaries =
                    CheckpointRing::compute_segment_boundaries(start_vi, tip_vi, max_div);

                let mut seg_start = start_vi;
                for &boundary in &boundaries {
                    if boundary <= seg_start || boundary > tip_vi {
                        continue;
                    }

                    // Render segment.
                    let mut gpu_ctx = make_gpu_ctx!("brush-rerender-seg");
                    engine.render_from_stabilized_range_to(&mut gpu_ctx, seg_start, boundary);
                    gpu_ctx.submit_final();

                    // Save checkpoint at this boundary.
                    if let Some(bbox) = engine.save_points.full_bbox() {
                        let sp_idx = engine.save_points.len().saturating_sub(1);
                        let render_state = engine.capture_render_state();
                        self.gpu.encode("checkpoint-save", |encoder| {
                            self.checkpoint_ring.save(
                                &self.gpu.device,
                                encoder,
                                stroke_buffer.stroke_texture(),
                                sp_idx,
                                boundary,
                                bbox,
                                render_state,
                            );
                        });
                    }

                    seg_start = boundary + 1;
                }

                // Render any remaining dabs past the last boundary.
                if seg_start <= tip_vi {
                    let mut gpu_ctx = make_gpu_ctx!("brush-rerender-tail");
                    engine.render_from_stabilized_range_to(&mut gpu_ctx, seg_start, tip_vi);
                    gpu_ctx.submit_final();
                }
            } else {
                // No divergence — render tail only.
                let mut gpu_ctx = make_gpu_ctx!("brush-dab");
                engine.render_from_stabilized_tail(&mut gpu_ctx);
                gpu_ctx.submit_final();

                // Periodically save a checkpoint to keep the ring fresh.
                let spacing = CheckpointRing::spacing(max_div);
                let should_save = match self.checkpoint_ring.newest_vector_index() {
                    Some(newest_vi) => tip_vi.saturating_sub(newest_vi) >= spacing,
                    None => true,
                };
                if should_save && !engine.save_points.is_empty() {
                    if let Some(bbox) = engine.save_points.full_bbox() {
                        let sp_idx = engine.save_points.len() - 1;
                        let render_state = engine.capture_render_state();
                        self.gpu.encode("checkpoint-save", |encoder| {
                            self.checkpoint_ring.save(
                                &self.gpu.device,
                                encoder,
                                stroke_buffer.stroke_texture(),
                                sp_idx,
                                tip_vi,
                                bbox,
                                render_state,
                            );
                        });
                    }
                }
            }

            // Ask the terminal to commit the stroke state onto the layer.
            // For paint this is `source_over(scratch × opacity, pre_stroke)`;
            // other terminals (warp, smudge, …) will do their own thing.
            {
                let mut gpu_ctx = make_gpu_ctx!("brush-commit");
                engine.commit(&mut gpu_ctx);
                gpu_ctx.submit_final();
            }
        } else {
            // Fallback: no stroke buffer — render directly to layer (shouldn't
            // happen in practice). Skips the lifecycle hooks since there's no
            // scratch to clear or commit.
            let layer_tex = if mask_editing {
                self.compositor.mask_texture(layer_id)
            } else {
                self.compositor.layer_texture(layer_id)
            };
            if let Some(layer_tex) = layer_tex {
                let canvas_view = layer_tex.view.clone();
                let canvas_texture = &layer_tex.texture;
                let mut gpu_ctx = BrushGpuContext {
                    encoder: self.gpu.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor {
                            label: Some("brush-dab"),
                        },
                    ),
                    device: &self.gpu.device,
                    queue: &self.gpu.queue,
                    dab_pool: &mut self.dab_pool,
                    pipelines: &self.brush_pipelines,
                    stroke_scratch_view: &canvas_view,
                    stroke_scratch_texture: canvas_texture,
                    canvas_width: canvas_w,
                    canvas_height: canvas_h,
                    layer_width: layer_tex.width,
                    layer_height: layer_tex.height,
                    layer_offset_x: layer_tex.offset_x,
                    layer_offset_y: layer_tex.offset_y,
                    selection_bind_group: sel_bg,
                    resource_handles: &self.resource_handles,
                    blend_mode: self.brush_blend_mode,
                    canvas_copy_origin: None,
                    preview_mask_view: None,
                    preview_mask_size: (0, 0),
                    brush_preview_info: None,
                    layer_view: None,
                    layer_texture: None,
                    pre_stroke_texture: None,
                    pre_stroke_bind_group: None,
                    scratch_bind_group: None,
                    dab_write_bbox: None,
                };
                self.brush_pipelines.reset_uniform_rings();
                engine.move_to(info, &mut gpu_ctx);
                gpu_ctx.submit_final();
            }
        }

        // Put the engine and buffer back.
        self.brush_stroke_engine = Some(engine);
        self.stroke_buffer = stroke_buffer;
    }

    /// Start async GPU flood fill: readback layer texture, then complete on a
    /// subsequent frame when the data arrives.
    fn gpu_flood_fill(
        &mut self,
        layer_id: u64,
        mask_editing: bool,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
        canvas_w: u32,
        canvas_h: u32,
    ) {
        let (texture, format) = if mask_editing {
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

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("flood-fill-readback"),
            });
        let request = readback::request_readback(
            &self.gpu.device,
            &mut encoder,
            texture,
            format,
            [0, 0, canvas_w, canvas_h],
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.readbacks.submit(
            request,
            ReadbackContext::FloodFill {
                layer_id,
                mask_editing,
                seed_x,
                seed_y,
                color,
                tolerance,
                canvas_w,
                canvas_h,
            },
        );
    }

    /// Complete a pending flood fill once readback data is available.
    pub(crate) fn complete_flood_fill(
        &mut self,
        layer_id: u64,
        mask_editing: bool,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
        canvas_w: u32,
        canvas_h: u32,
        pixels: Vec<u8>,
    ) {
        // 1. CPU scanline fill → produce R8 mask.
        let fill_mask = if mask_editing {
            flood_fill::flood_fill_r8(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
        } else {
            flood_fill::flood_fill_rgba(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
        };

        // 2. Combine fill mask with active selection (if any), then upload.
        let effective_mask = if self.gpu_selection.active {
            if let Some(sel) = &self.gpu_selection.cpu_cache {
                fill_mask
                    .iter()
                    .zip(sel.iter())
                    .map(|(&f, &s)| ((f as u16 * s as u16) / 255) as u8)
                    .collect()
            } else {
                fill_mask
            }
        } else {
            fill_mask
        };

        let mask_bind_group = self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device,
            &self.gpu.queue,
            canvas_w,
            canvas_h,
            &effective_mask,
            "flood-fill-mask",
        );

        let (target, _) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        self.gpu.encode("flood-fill-stamp", |encoder| {
            target.fill_rect_with_selection(
                encoder,
                &self.paint_pipelines,
                &self.gpu.queue,
                [0, 0, canvas_w as i32, canvas_h as i32],
                color,
                &mask_bind_group,
            );
        });

        // 4. Commit undo — use full canvas rect (flood fill can change any pixel).
        let format = if mask_editing {
            wgpu::TextureFormat::R8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let rect = [0u32, 0, canvas_w, canvas_h];
        self.gpu.encode("flood-fill-undo", |encoder| {
            let entry = self
                .region_store
                .commit_region(encoder, layer_id, format, rect);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
        self.scratch_saved = false;

        self.compositor.mark_dirty();
    }

    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            // If a flood fill is pending, defer undo commit — complete_flood_fill
            // will handle it when the readback arrives.
            if self
                .readbacks
                .any(|c| matches!(c, ReadbackContext::FloodFill { .. }))
            {
                self.doc.set_mask_editing(None);
                return;
            }

            // Finalize brush stroke engine and destroy stroke buffer + checkpoints.
            if let Some(engine) = self.brush_stroke_engine.take() {
                let _record = engine.end();
            }
            self.stroke_buffer = None;
            self.checkpoint_ring.clear();

            // Dispatch GPU diff to find the exact changed region for undo.
            if self.scratch_saved && self.pending_undo_commit.is_none() {
                let format = if self.editing_mask_layer == Some(layer_id) {
                    wgpu::TextureFormat::R8Unorm
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                };
                let current_view = if self.editing_mask_layer == Some(layer_id) {
                    self.compositor.mask_texture(layer_id).map(|t| &t.view)
                } else {
                    self.compositor.layer_texture(layer_id).map(|t| &t.view)
                };
                if let Some(current_view) = current_view {
                    let scratch_view = self.region_store.scratch_view(format);
                    let (w, h) = self.region_store.scratch_dimensions();
                    self.diff_rect.request(
                        &self.gpu.device,
                        &self.gpu.queue,
                        &scratch_view,
                        current_view,
                        w,
                        h,
                    );
                    self.pending_undo_commit = Some(PendingUndoCommit { layer_id, format });
                }
            }
            self.scratch_saved = false;
            self.doc.set_mask_editing(None);
        }
    }

    // --- GPU erase helpers ---

    /// Clear layer pixels within the current selection via GPU erase pass.
    pub(crate) fn gpu_clear_selection(&mut self, layer_id: u64) {
        if !self.gpu_selection.active {
            return;
        }

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let mask_editing = self.editing_mask_layer == Some(layer_id);

        let (target, format) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        // Save region for undo.
        self.gpu.encode("clear-sel-save", |encoder| {
            self.region_store.save_region(
                encoder,
                target.texture,
                format,
                [0, 0, canvas_w, canvas_h],
            );
        });

        // Erase within selection using the cached GPU selection bind group.
        let (target, _) = self.get_paint_target(layer_id, mask_editing).unwrap();
        let sel_bg = self.gpu_selection.paint_bind_group();
        self.gpu.encode("clear-sel-erase", |encoder| {
            target.erase_with_selection(encoder, &self.paint_pipelines, &self.gpu.queue, sel_bg);
        });

        // Commit for undo.
        self.gpu.encode("clear-sel-commit", |encoder| {
            let entry = self.region_store.commit_region(
                encoder,
                layer_id,
                format,
                [0, 0, canvas_w, canvas_h],
            );
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
        self.compositor.mark_dirty();
    }

    /// Clear entire layer to transparent via GPU.
    pub(crate) fn gpu_clear_layer(&mut self, layer_id: u64) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let mask_editing = self.editing_mask_layer == Some(layer_id);

        let (target, format) = match self.get_paint_target(layer_id, mask_editing) {
            Some(t) => t,
            None => return,
        };

        // Save region for undo.
        self.gpu.encode("clear-layer-save", |encoder| {
            self.region_store.save_region(
                encoder,
                target.texture,
                format,
                [0, 0, canvas_w, canvas_h],
            );
        });

        // Clear the full canvas.
        let (target, _) = self.get_paint_target(layer_id, mask_editing).unwrap();
        self.gpu.encode("clear-layer", |encoder| {
            target.clear_rect(
                encoder,
                &self.paint_pipelines,
                &self.gpu.queue,
                [0, 0, canvas_w as i32, canvas_h as i32],
            );
        });

        // Commit for undo.
        self.gpu.encode("clear-layer-commit", |encoder| {
            let entry = self.region_store.commit_region(
                encoder,
                layer_id,
                format,
                [0, 0, canvas_w, canvas_h],
            );
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
        self.compositor.mark_dirty();
    }

    /// Get a GpuPaintTarget for a layer (or its mask), plus its format.
    pub(crate) fn get_paint_target(
        &self,
        layer_id: u64,
        mask_editing: bool,
    ) -> Option<(GpuPaintTarget<'_>, wgpu::TextureFormat)> {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        if mask_editing {
            self.compositor.mask_texture(layer_id).map(|t| {
                (
                    GpuPaintTarget::from_mask(t, canvas_w, canvas_h),
                    wgpu::TextureFormat::R8Unorm,
                )
            })
        } else {
            self.compositor.layer_texture(layer_id).map(|t| {
                (
                    GpuPaintTarget::from_layer(t, canvas_w, canvas_h),
                    wgpu::TextureFormat::Rgba8Unorm,
                )
            })
        }
    }

    /// Upload a cropped region of the GPU selection as an R8 texture bind group.
    /// Reads from the CPU cache (populated by async readback or eagerly on upload).
    pub(crate) fn upload_cropped_selection_r8(
        &self,
        origin: (i32, i32),
        width: u32,
        height: u32,
    ) -> Option<wgpu::BindGroup> {
        if !self.gpu_selection.active {
            return None;
        }

        let full = self.gpu_selection.cpu_cache.as_ref()?;
        let (ox, oy) = origin;
        let cw = self.gpu_selection.width;
        let ch = self.gpu_selection.height;

        let mut pixels = vec![0u8; (width * height) as usize];
        for py in 0..height {
            for px in 0..width {
                let sx = ox + px as i32;
                let sy = oy + py as i32;
                if sx >= 0 && sy >= 0 && (sx as u32) < cw && (sy as u32) < ch {
                    pixels[(py * width + px) as usize] =
                        full[(sy as u32 * cw + sx as u32) as usize];
                }
            }
        }

        Some(self.paint_pipelines.upload_r8_bind_group(
            &self.gpu.device,
            &self.gpu.queue,
            width,
            height,
            &pixels,
            "selection-cropped",
        ))
    }
}
