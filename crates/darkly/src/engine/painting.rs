//! Stroke lifecycle, flood fill, erase helpers, and paint infrastructure.

use super::types::StrokeOp;
use super::{DarklyEngine, PendingUndoCommit, ReadbackContext};
use crate::brush::checkpoint_ring::CheckpointRing;
use crate::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
use crate::brush::paint_info::PaintInformation;
use crate::brush::spacing::SpacingConfig;
use crate::brush::stroke_buffer::StrokeBuffer;
use crate::brush::stroke_engine::StrokeEngine;
use crate::gpu::flood_fill;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback;
use crate::layer::LayerId;
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
            // Diff not ready — fall back to the full saved area. (NOT
            // `scratch_dimensions()` — those diverge from `snapshot.saved`
            // after a mid-stroke `grow_scratch_preserving`.)
            None => commit.snapshot.saved,
        };

        let layer_frame = match self.compositor.node_texture(commit.layer_id) {
            Some(t) => t.canvas_frame(),
            None => return,
        };

        self.gpu.encode("brush-stroke-end-flush", |encoder| {
            let entry = self.region_store.commit_region(
                encoder,
                commit.layer_id,
                &layer_frame,
                &commit.snapshot,
                rect,
            );
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
    }

    // --- Painting ---

    /// Fill the layer with the default background image, centered and clipped
    /// to the canvas. The image is baked into the binary at build time.
    pub fn fill_background(&mut self, layer_id: LayerId) {
        const IMAGE_BYTES: &[u8] = include_bytes!("../../resources/backgrounds/quiet-night.jpg");

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let rect = crate::coord::CanvasRect::from_xywh(0, 0, canvas_w, canvas_h);
        let format = wgpu::TextureFormat::Rgba8Unorm;

        let layer_tex = match self.compositor.node_texture(layer_id) {
            Some(t) => t,
            None => return,
        };
        let layer_frame = layer_tex.canvas_frame();

        // Save current state to scratch for undo.
        self.gpu.encode("fill-background-save", |encoder| {
            let snap = self.region_store.save_region(
                &self.gpu.device,
                encoder,
                &layer_frame,
                format,
                rect,
            );
            let entry =
                self.region_store
                    .commit_region(encoder, layer_id, &layer_frame, &snap, rect);
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
            let layer_tex = self.compositor.node_texture(layer_id).unwrap();
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

        self.compositor.mark_node_pixels_dirty(layer_id);
    }

    // --- Stroke lifecycle ---
    // The active node id directly identifies the paint target — for a mask
    // modifier id, paint goes to the mask's R8 PixelBuffer; for a raster id,
    // paint goes to the RGBA layer texture. No sidecar redirect.
    //
    // All stroke ops go through GPU render passes.

    pub fn begin_stroke(&mut self, layer_id: LayerId) {
        self.auto_commit_floating();
        self.active_stroke_layer = Some(layer_id);
        // Reset the per-stroke perf accumulator. Emitted at `end_stroke`.
        self.stroke_perf = super::perf::StrokePerfStats::default();
        // GPU setup is deferred to first stroke_to (lazy init).
    }

    /// Feed the largest BrushStroke backlog the WASM bridge saw in its most
    /// recent drain. Lets the stroke summary include input-queueing signal
    /// without exposing the perf struct across the crate boundary.
    pub fn record_input_backlog(&mut self, backlog: u32) {
        if backlog > self.stroke_perf.max_queue_backlog {
            self.stroke_perf.max_queue_backlog = backlog;
        }
    }

    /// Read the most recent `render()` sub-phase timings. Used by the WASM
    /// bridge's slow-frame log to break down where the time went.
    pub fn last_render_phases(&self) -> super::FrameRenderPhases {
        self.last_frame_phases
    }

    pub fn stroke_to(&mut self, op: StrokeOp) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };
        self.gpu_stroke_to(layer_id, op);
    }

    /// GPU paint path for all stroke operations.
    fn gpu_stroke_to(&mut self, layer_id: LayerId, op: StrokeOp) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        // Brush strokes may extend past the layer's current canvas extent
        // (e.g. paste-extent layers, or any stroke that wanders past the
        // canvas). Grow the layer texture in chunked steps so the dab
        // dispatch and undo paths see a sufficiently-large layer.
        // Non-BrushStroke ops (gradient, flood fill, fill rect) operate on
        // existing pixels and don't need preemptive growth.
        if let StrokeOp::BrushStroke { x, y, .. } = op {
            self.ensure_layer_covers_dab(layer_id, x, y);
        }

        // Lazy init: save the paint target to scratch for undo on first
        // stroke_to. Uses the target's actual texture dimensions (not canvas)
        // so paste-extent layers preserve off-canvas pixels through undo.
        // The unified node-texture pool dispatches by node id; format dispatch
        // is read off `LayerTexture.format` rather than a sidecar boolean.
        if self.scratch_snapshot.is_none() {
            self.flush_pending_undo_commit();
            // Inline dispatch: the borrow checker treats `self.paint_target(...)`
            // as borrowing all of &self, which conflicts with the
            // &mut self.region_store call below. Direct field access via
            // `self.compositor.node_texture(...)` borrows only that sub-field,
            // so split borrowing of `region_store` works.
            let (frame, format) = match self.compositor.node_texture(layer_id) {
                Some(t) => (t.canvas_frame(), t.format),
                None => return,
            };

            let saved_rect = frame.canvas_extent;
            let snap = self.gpu.encode_ret("stroke-begin", |encoder| {
                self.region_store
                    .save_region(&self.gpu.device, encoder, &frame, format, saved_rect)
            });
            self.scratch_snapshot = Some(snap);
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
                let target = match self.paint_target(layer_id) {
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

        // The compositor needs to recomposite the layer's pixels on
        // the next render; thumbnail invalidation lives at the stroke
        // boundary (end_stroke) rather than per-segment so the panel
        // updates once per stroke instead of mid-flight.
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
    /// The `needed` rect padded outward by `DAB_REFERENCE_SIZE/2` so the new
    /// chunk-aligned extent comfortably covers the dab's worst-case
    /// footprint, not just its center pixel.
    fn ensure_layer_covers_dab(&mut self, layer_id: LayerId, x: f32, y: f32) {
        // Fetch the current paint-target extent before mutating the compositor.
        // `paint_target()` resolves the node id against the unified texture
        // pool; format dispatch lives behind that interface.
        let current_extent = match self.paint_target(layer_id) {
            Some(t) => t.canvas_frame().canvas_extent,
            None => return,
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
        // DAB_REFERENCE_SIZE so the grown extent includes the dab's footprint.
        const HALF: i32 = (crate::brush::dab_pool::DAB_REFERENCE_SIZE / 2) as i32;
        let needed = crate::coord::CanvasRect::from_xywh(
            cx - HALF,
            cy - HALF,
            (HALF as u32) * 2,
            (HALF as u32) * 2,
        );

        let new_extent = match self.grow_layer(layer_id, needed) {
            Some(e) => e,
            None => return,
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
                    self.brush_pipelines.canvas_copy_bind_group_layout(),
                );
            });
        }

        // The brush engine's bbox metadata (`save_points`, `checkpoint_ring`)
        // is in canvas coords (Storage Frame Rule). Canvas coords are stable
        // across layer growth, so no metadata patch is needed — only the GPU
        // textures got rebased above, and the metadata translates to the new
        // layer-local frame on demand at the wgpu boundary.

        // Re-anchor the region_store scratch so the diff_rect at end_stroke
        // compares matching coordinate frames. If the scratch hasn't been
        // saved yet (this is the first dab and lazy init hasn't run), the
        // rebase is a no-op on still-empty contents.
        if let Some(snap) = self.scratch_snapshot.as_mut() {
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
            // After grow_scratch_preserving, the new scratch holds:
            //   - old layer contents at (dx, dy) (translated from old origin)
            //   - zero-init in the newly-grown canvas regions
            // Both are correct pre-stroke state — the newly-grown pixels
            // didn't exist before the grow, so "zero / transparent" IS
            // their pre-stroke value. Widen `saved` to cover the full
            // new canvas extent so a diff_rect that spills into the
            // newly-grown area is still contained at commit time.
            snap.saved = new_extent;
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
    }

    /// Grow whichever raster layer owns `node_id` to cover `needed`.
    ///
    /// - If `node_id` is a raster layer, grows it directly.
    /// - If `node_id` is a modifier (e.g. a mask), grows its host raster —
    ///   which lockstep-grows the modifier alongside it.
    ///
    /// Lets callers that hold a generic node id (transform commit, paste
    /// commit) request growth without first disambiguating between raster
    /// and modifier ids.
    pub(crate) fn grow_node_to_fit(
        &mut self,
        node_id: crate::layer::LayerId,
        needed: crate::coord::CanvasRect,
    ) -> Option<crate::coord::CanvasRect> {
        let target_id = if self.doc.is_modifier(node_id) {
            self.doc.parent_of(node_id)?
        } else {
            node_id
        };
        self.grow_layer(target_id, needed)
    }

    /// Grow a raster layer's bounds to cover `needed` (canvas-space).
    ///
    /// Document-led: writes `RasterLayer.bounds` first, then resizes the
    /// compositor's GPU texture to match and refreshes blend uniforms.
    /// Returns `Some(new_extent)` if the layer was actually grown,
    /// `None` if no growth was needed or the cap was hit.
    pub(crate) fn grow_layer(
        &mut self,
        layer_id: LayerId,
        needed: crate::coord::CanvasRect,
    ) -> Option<crate::coord::CanvasRect> {
        use crate::gpu::compositor::{LAYER_GROWTH_CHUNK, MAX_LAYER_DIM};

        let current = match self.doc.layer(layer_id) {
            Some(crate::layer::Layer::Raster(r)) => r.pixels.bounds,
            _ => return None,
        };
        if current.contains(needed) {
            return None;
        }

        let new_extent = current.union(needed).round_outward(LAYER_GROWTH_CHUNK);

        if new_extent.width > MAX_LAYER_DIM || new_extent.height > MAX_LAYER_DIM {
            if !self.layer_growth_capped {
                self.layer_growth_capped = true;
                log::warn!(
                    "Layer {:?} growth refused: requested {}×{} exceeds MAX_LAYER_DIM ({})",
                    layer_id,
                    new_extent.width,
                    new_extent.height,
                    MAX_LAYER_DIM,
                );
            }
            return None;
        }

        // Doc first — the layer's `PixelBuffer` is the source of truth.
        let isolated = self.host_renders_isolated(layer_id);
        let (opacity, blend_mode_gpu) = match self.doc.layer_mut(layer_id) {
            Some(crate::layer::Layer::Raster(r)) => {
                r.pixels.bounds = new_extent;
                (r.blend.opacity, r.blend.blend_mode.gpu_value)
            }
            _ => return None,
        };

        // Encoder discipline: the resize must run in its own encoder,
        // submitted before any subsequent dab dispatch can start a new
        // encoder against the new texture. `gpu.encode` already does
        // one-encoder-per-call.
        // Lockstep growth: collect mask-modifier ids of this host that
        // share the host's UV space (i.e. non-locked) and grow them in the
        // same encoder. Locked modifiers stay at their old extent — the
        // shader samples each `PixelBuffer` via its own bounds, so a
        // diverged modifier just renders at its frozen position.
        let lockstep_ids: Vec<LayerId> = self
            .doc
            .modifiers_of(layer_id)
            .iter()
            .copied()
            .filter(|mid| {
                self.doc
                    .find_modifier(*mid)
                    .map(|m| !m.common.locked && m.pixels().is_some())
                    .unwrap_or(false)
            })
            .collect();

        self.gpu.encode("layer-grow", |encoder| {
            self.compositor.resize_node_texture(
                &self.gpu.device,
                &self.gpu.queue,
                encoder,
                layer_id,
                new_extent,
            );
            for mod_id in &lockstep_ids {
                self.compositor.resize_node_texture(
                    &self.gpu.device,
                    &self.gpu.queue,
                    encoder,
                    *mod_id,
                    new_extent,
                );
            }
        });

        // Sync each lockstep modifier's document `PixelBuffer.bounds` to
        // match its newly-grown GPU texture. The compositor is the realization;
        // the document is the source of truth — both must agree post-resize.
        for mod_id in &lockstep_ids {
            if let Some(modifier) = self.doc.find_modifier_mut(*mod_id) {
                if let Some(buf) = modifier.pixels_mut() {
                    buf.bounds = new_extent;
                }
            }
        }

        // Refresh the blend-uniform buffer so the composite pass sees the
        // new offset/size on the next render.
        self.compositor.update_raster_uniforms_full(
            &self.gpu.queue,
            layer_id,
            opacity,
            blend_mode_gpu,
            isolated,
        );

        Some(new_extent)
    }

    /// Handle a BrushStroke event through the node-graph brush engine.
    ///
    /// Lazy-inits a `StrokeEngine` + `StrokeBuffer` on the first event.
    /// Each event feeds through the stabilizer, which may trigger rewind
    /// and re-rendering of the stroke from scratch.
    fn brush_stroke_to(
        &mut self,
        layer_id: LayerId,
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
            // Inline dispatch (vs `self.paint_target(...)`) so the borrow of
            // `self.compositor.node_textures[id]` is at the field level —
            // letting `&self.gpu`, `&self.dab_pool`, etc. be borrowed
            // alongside it without conflict.
            let layer_tex = self.compositor.node_texture(layer_id);
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
                    &self.brush_pipelines,
                );
                let paint_target = GpuPaintTarget::from_node(layer_tex, canvas_w, canvas_h);
                self.gpu.encode("stroke-buffer-init", |encoder| {
                    stroke_buffer.save_pre_stroke(
                        &self.gpu.device,
                        encoder,
                        &self.brush_pipelines,
                        &paint_target,
                    );
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

        // Get the paint target (layer or mask) — encapsulates format and
        // brush-side commit dispatch so the brush stack stays format-agnostic.
        // Inline dispatch (vs `self.paint_target(...)`) for borrow-checker
        // reasons — the BrushGpuContext construction below needs &mut
        // self.dab_pool alongside this borrow.
        let layer_tex = match self.compositor.node_texture(layer_id) {
            Some(t) => t,
            None => return,
        };
        let paint_target = GpuPaintTarget::from_node(layer_tex, canvas_w, canvas_h);

        // Take the stroke engine and buffer out to avoid borrow conflicts.
        let mut engine = self.brush_stroke_engine.take().unwrap();
        let mut stroke_buffer = self.stroke_buffer.take();

        let sel_bg = if self.has_selection() {
            self.compositor
                .selection_state()
                .map(|s| s.brush_bind_group())
                .unwrap_or(&self.brush_pipelines.default_selection_bind_group)
        } else {
            &self.brush_pipelines.default_selection_bind_group
        };

        // Per-event host wall-clock. Captures the full
        // `gpu_stroke_to(BrushStroke{..})` invocation so the stroke summary
        // can report avg ms/event.
        let perf_start = web_time::Instant::now();
        // Track dabs-placed delta across all contexts spawned by this event,
        // so the per-event peak is comparable across events.
        let dabs_before = self.stroke_perf.total_dabs;

        if let Some(ref mut stroke_buffer) = stroke_buffer {
            // Stabilized path: dabs render into the scratch, then the
            // terminal's `commit` hook lands them on the layer.
            self.brush_pipelines.reset_uniform_rings();
            let t_stab = web_time::Instant::now();
            let result = engine.stabilize(info);
            let max_div = engine.max_divergence_window();
            let tip_vi = engine.stabilizer_len().saturating_sub(1);
            self.stroke_perf.total_phase_stabilize_us += t_stab.elapsed().as_micros() as u64;

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

            // The checkpoint ring's coverage invariant depends on
            // `max_divergence_window` being a true upper bound on
            // `tip_vi - find_divergence().unwrap()`. Make any future drift
            // between the stabilizer's bound and its detector loud in debug
            // builds. (The synthetic tip-divergence path is always within
            // bound by construction, but `result.divergence_index` is what
            // the stabilizer reported.)
            #[cfg(debug_assertions)]
            if let Some(k) = result.divergence_index {
                let earliest = tip_vi.saturating_sub(max_div);
                debug_assert!(
                    k >= earliest,
                    "stabilizer returned divergence_index={k} but max_div={max_div} \
                     requires >= {earliest} (tip_vi={tip_vi})",
                );
            }

            // Helper macro: create a BrushGpuContext wired with the stroke
            // scratch, paint target (layer or mask), and pre-stroke snapshot.
            // The paint target carries the destination format internally;
            // `color_output::commit` calls `paint_target.commit_brush_dab(...)`
            // and never branches on R8 vs RGBA8.
            macro_rules! make_gpu_ctx {
                ($label:expr) => {{
                    // Re-borrow per invocation: each ctx holds &mut Scratch
                    // for its own lifetime, then is consumed by `submit_final()`
                    // before the next macro expansion reborrows.
                    let (scratch, pre_stroke_texture, pre_stroke_bind_group) =
                        stroke_buffer.parts_for_brush_ctx();
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
                        scratch: Some(scratch),
                        canvas_width: canvas_w,
                        canvas_height: canvas_h,
                        paint_target: Some(paint_target),
                        selection_bind_group: sel_bg,
                        preview_target_view: None,
                        resource_handles: &self.resource_handles,
                        // blend_mode applies at commit (paint vs. erase). The
                        // per-dab composite inside `color_output::evaluate_gpu`
                        // hard-codes source-over regardless of this value.
                        blend_mode: self.brush_blend_mode,
                        preview_mask_view: None,
                        preview_mask_size: (0, 0),
                        brush_preview_info: None,
                        pre_stroke_texture: Some(pre_stroke_texture),
                        pre_stroke_bind_group: Some(pre_stroke_bind_group),
                        dab_write_canvas_bbox: None,
                        perf: BrushPerfCounters::default(),
                    }
                }};
            }

            // First event of the stroke — let the terminal set up its scratch.
            if need_begin_stroke {
                let mut gpu_ctx = make_gpu_ctx!("brush-begin-stroke");
                engine.begin_stroke(&mut gpu_ctx);
                let p = gpu_ctx.submit_final();
                self.stroke_perf.merge_brush(p);
            }

            if let Some(div_idx) = div_idx {
                self.stroke_perf.divergence_events += 1;
                // Divergence — try checkpoint-based partial re-render.
                // The terminal's `begin_stroke` establishes outside-bbox
                // state for whichever path we take below; the checkpoint
                // ring no longer clears on its own.
                let t_rewind = web_time::Instant::now();
                {
                    let mut gpu_ctx = make_gpu_ctx!("brush-begin-stroke-rewind");
                    engine.begin_stroke(&mut gpu_ctx);
                    let p = gpu_ctx.submit_final();
                    self.stroke_perf.merge_brush(p);
                }
                self.stroke_perf.total_phase_rewind_us += t_rewind.elapsed().as_micros() as u64;

                let stroke_frame = crate::gpu::atlas::CanvasFrame {
                    texture: stroke_buffer.scratch().write_texture(),
                    canvas_extent: paint_target.canvas_frame().canvas_extent,
                };
                let t_restore = web_time::Instant::now();
                let restore = self.gpu.encode_ret("stroke-checkpoint-restore", |encoder| {
                    self.checkpoint_ring
                        .restore_before(encoder, &stroke_frame, div_idx)
                });
                self.stroke_perf.total_phase_restore_us += t_restore.elapsed().as_micros() as u64;
                self.stroke_perf.total_submits += 1;

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
                    // No checkpoint before divergence — full re-render.
                    //
                    // Two cases here. If the ring had valid slots but none
                    // satisfied `vi < div_idx`, the coverage invariant has
                    // failed (the architectural defect the ring's eviction
                    // policy is designed to prevent). If the ring was
                    // empty, this is initialization — the first divergence
                    // event of the stroke, before any checkpoint has been
                    // saved — which is structurally unavoidable and cheap
                    // (`tip_vi` is small, so the re-render is short).
                    // Only the former is a "mid-stroke full re-render
                    // fallback" worth counting.
                    if self.checkpoint_ring.has_any_valid() {
                        self.stroke_perf.full_rerender_events += 1;
                    }
                    engine.reset_render_state();
                    self.checkpoint_ring.clear();
                    0
                };
                let rerender_range = tip_vi.saturating_sub(start_vi) as u32;
                self.stroke_perf.total_rerender_range += rerender_range as u64;
                if rerender_range > self.stroke_perf.max_rerender_range {
                    self.stroke_perf.max_rerender_range = rerender_range;
                }
                self.stroke_perf.last_max_div_window = max_div;
                self.stroke_perf.last_spacing = CheckpointRing::spacing(max_div);

                // Render in segments with checkpoints at boundaries.
                let boundaries =
                    CheckpointRing::compute_segment_boundaries(start_vi, tip_vi, max_div);

                let t_segments = web_time::Instant::now();
                let mut seg_start = start_vi;
                for &boundary in &boundaries {
                    // Strict `<` (not `<=`): `compute_segment_boundaries`
                    // prepends a `vi=0` anchor when `start_vi=0`, and we
                    // need that single-vi segment `[0..=0]` to actually
                    // render + save its checkpoint rather than being
                    // skipped.
                    if boundary < seg_start || boundary > tip_vi {
                        continue;
                    }

                    // Render segment.
                    let mut gpu_ctx = make_gpu_ctx!("brush-rerender-seg");
                    engine.render_from_stabilized_range_to(&mut gpu_ctx, seg_start, boundary);
                    let p = gpu_ctx.submit_final();
                    self.stroke_perf.merge_brush(p);

                    // Save checkpoint at this boundary.
                    if let Some(bbox) = engine.save_points.full_bbox() {
                        let sp_idx = engine.save_points.len().saturating_sub(1);
                        let render_state = engine.capture_render_state();
                        let stroke_frame = crate::gpu::atlas::CanvasFrame {
                            texture: stroke_buffer.scratch().write_texture(),
                            canvas_extent: paint_target.canvas_frame().canvas_extent,
                        };
                        self.gpu.encode("checkpoint-save", |encoder| {
                            self.checkpoint_ring.save(
                                &self.gpu.device,
                                encoder,
                                &stroke_frame,
                                sp_idx,
                                boundary,
                                bbox,
                                render_state,
                                tip_vi,
                                max_div,
                            );
                        });
                        self.stroke_perf.total_submits += 1;
                    }

                    seg_start = boundary + 1;
                }
                self.stroke_perf.total_phase_segments_us += t_segments.elapsed().as_micros() as u64;

                // Render any remaining dabs past the last boundary.
                if seg_start <= tip_vi {
                    let t_tail = web_time::Instant::now();
                    let mut gpu_ctx = make_gpu_ctx!("brush-rerender-tail");
                    engine.render_from_stabilized_range_to(&mut gpu_ctx, seg_start, tip_vi);
                    let p = gpu_ctx.submit_final();
                    self.stroke_perf.merge_brush(p);
                    self.stroke_perf.total_phase_tail_us += t_tail.elapsed().as_micros() as u64;
                }
            } else {
                // No divergence — render tail only.
                let t_tail = web_time::Instant::now();
                let mut gpu_ctx = make_gpu_ctx!("brush-dab");
                engine.render_from_stabilized_tail(&mut gpu_ctx);
                let p = gpu_ctx.submit_final();
                self.stroke_perf.merge_brush(p);

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
                        let stroke_frame = crate::gpu::atlas::CanvasFrame {
                            texture: stroke_buffer.scratch().write_texture(),
                            canvas_extent: paint_target.canvas_frame().canvas_extent,
                        };
                        self.gpu.encode("checkpoint-save", |encoder| {
                            self.checkpoint_ring.save(
                                &self.gpu.device,
                                encoder,
                                &stroke_frame,
                                sp_idx,
                                tip_vi,
                                bbox,
                                render_state,
                                tip_vi,
                                max_div,
                            );
                        });
                        self.stroke_perf.total_submits += 1;
                    }
                }
                self.stroke_perf.total_phase_tail_us += t_tail.elapsed().as_micros() as u64;
                self.stroke_perf.last_max_div_window = max_div;
                self.stroke_perf.last_spacing = CheckpointRing::spacing(max_div);
            }

            // Ask the terminal to commit the stroke state onto the layer.
            // For paint this is `source_over(scratch × opacity, pre_stroke)`;
            // other terminals (warp, smudge, …) will do their own thing.
            let t_commit = web_time::Instant::now();
            {
                let mut gpu_ctx = make_gpu_ctx!("brush-commit");
                engine.commit(&mut gpu_ctx);
                let p = gpu_ctx.submit_final();
                self.stroke_perf.merge_brush(p);
            }
            self.stroke_perf.total_phase_commit_us += t_commit.elapsed().as_micros() as u64;
        } else {
            // Fallback: no stroke buffer — render directly to the paint
            // target (shouldn't happen in practice). Skips the lifecycle
            // hooks since there's no scratch to clear or commit. Inline
            // dispatch so the borrow of `self.compositor.X[id]` is at the
            // field level, leaving `&mut self.dab_pool` free.
            let layer_tex = self.compositor.node_texture(layer_id);
            if let Some(layer_tex) = layer_tex {
                let canvas_view = &layer_tex.view;
                let paint_target = GpuPaintTarget::from_node(layer_tex, canvas_w, canvas_h);
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
                    // No stroke buffer in this defensive fallback — `move_to`
                    // only updates stabilizer state and never reaches into
                    // scratch.  Anything that does would panic, which is the
                    // correct signal that the fallback was reached.
                    scratch: None,
                    canvas_width: canvas_w,
                    canvas_height: canvas_h,
                    paint_target: Some(paint_target),
                    selection_bind_group: sel_bg,
                    preview_target_view: Some(canvas_view),
                    resource_handles: &self.resource_handles,
                    blend_mode: self.brush_blend_mode,
                    preview_mask_view: None,
                    preview_mask_size: (0, 0),
                    brush_preview_info: None,
                    pre_stroke_texture: None,
                    pre_stroke_bind_group: None,
                    dab_write_canvas_bbox: None,
                    perf: BrushPerfCounters::default(),
                };
                self.brush_pipelines.reset_uniform_rings();
                engine.move_to(info, &mut gpu_ctx);
                let _ = gpu_ctx.submit_final();
            }
        }

        // Put the engine and buffer back.
        self.brush_stroke_engine = Some(engine);
        self.stroke_buffer = stroke_buffer;

        // Record per-event timing + per-event dab count peak. Sub-phase
        // counters were accumulated in-place above; the wrap-up here is
        // just per-event aggregation.
        let elapsed_us = perf_start.elapsed().as_micros() as u64;
        self.stroke_perf.events += 1;
        self.stroke_perf.total_elapsed_us += elapsed_us;
        if elapsed_us > self.stroke_perf.max_event_us {
            self.stroke_perf.max_event_us = elapsed_us;
        }
        let dabs_this_event = (self.stroke_perf.total_dabs - dabs_before) as u32;
        self.stroke_perf.update_max_dabs_per_event(dabs_this_event);
    }

    /// Start async GPU flood fill: readback paint target texture, then
    /// complete on a subsequent frame when the data arrives.
    fn gpu_flood_fill(
        &mut self,
        layer_id: LayerId,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
        canvas_w: u32,
        canvas_h: u32,
    ) {
        let pt = match self.paint_target(layer_id) {
            Some(t) => t,
            None => return,
        };
        let texture = pt.texture;
        let format = pt.format;

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
                node_id: layer_id,
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
    /// Format-driven: the node id resolves to either an R8 mask or an RGBA
    /// layer texture, and the CPU-side scanline fill picks the matching
    /// flood-fill variant from the texture's format.
    pub(crate) fn complete_flood_fill(
        &mut self,
        layer_id: LayerId,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
        canvas_w: u32,
        canvas_h: u32,
        pixels: Vec<u8>,
    ) {
        let format = self
            .compositor
            .node_texture(layer_id)
            .map(|t| t.format)
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);
        let fill_mask = match format {
            wgpu::TextureFormat::R8Unorm => {
                flood_fill::flood_fill_r8(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
            }
            _ => {
                flood_fill::flood_fill_rgba(&pixels, canvas_w, canvas_h, seed_x, seed_y, tolerance)
            }
        };

        // 2. Combine fill mask with active selection (if any), then upload.
        let effective_mask = if self.has_selection() {
            if let Some(sel) = self.selection_cpu_cache() {
                fill_mask
                    .iter()
                    .zip(sel.iter())
                    .map(|(&f, &s)| ((f as u16 * s as u16) / 255) as u8)
                    .collect::<Vec<u8>>()
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

        let target = match self.paint_target(layer_id) {
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

        // 4. Commit undo. The lazy save in `gpu_stroke_to` populated
        //    `scratch_snapshot` with the full layer; flood fill can change
        //    any pixel inside the canvas, so commit the canvas-sized
        //    sub-rect of that snapshot.
        let snap = match self.scratch_snapshot.take() {
            Some(s) => s,
            // No snapshot means the lazy save never ran (stroke_to was
            // never called for this op) — extremely unusual; bail rather
            // than fabricate an empty snapshot.
            None => {
                self.compositor.mark_node_pixels_dirty(layer_id);
                return;
            }
        };
        let layer_frame = match self.compositor.node_texture(layer_id) {
            Some(t) => t.canvas_frame(),
            None => {
                self.compositor.mark_node_pixels_dirty(layer_id);
                return;
            }
        };
        let rect = crate::coord::CanvasRect::from_xywh(0, 0, canvas_w, canvas_h);
        self.gpu.encode("flood-fill-undo", |encoder| {
            let entry =
                self.region_store
                    .commit_region(encoder, layer_id, &layer_frame, &snap, rect);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });

        self.compositor.mark_node_pixels_dirty(layer_id);
    }

    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            // Emit the per-stroke perf summary before any cleanup. `events == 0`
            // means this wasn't a brush stroke (e.g. a flood-fill `end_stroke`)
            // so skip the log.
            let p = &self.stroke_perf;
            if p.events > 0 {
                let n = p.events as f32;
                let to_ms = |v: u64| (v as f32) / 1000.0;
                let avg_us = p.total_elapsed_us as f32 / n;
                let avg_rerender = if p.divergence_events > 0 {
                    p.total_rerender_range as f32 / p.divergence_events as f32
                } else {
                    0.0
                };
                log::info!(
                    "[stab-perf] stroke summary: events={} div_events={} full_rerenders={} \
                     avg_event={:.2}ms max_event={:.2}ms total={:.0}ms \
                     avg_rerender_range={:.1} max_rerender_range={} \
                     max_div_window={} spacing={} max_queue_backlog={} \
                     total_dabs={} max_dabs/event={}",
                    p.events,
                    p.divergence_events,
                    p.full_rerender_events,
                    avg_us / 1000.0,
                    p.max_event_us as f32 / 1000.0,
                    p.total_elapsed_us as f32 / 1000.0,
                    avg_rerender,
                    p.max_rerender_range,
                    p.last_max_div_window,
                    p.last_spacing,
                    p.max_queue_backlog,
                    p.total_dabs,
                    p.max_dabs_per_event,
                );
                // Phase breakdown — averaged per event.
                log::info!(
                    "[stab-perf] phases (avg per event): \
                     stabilize={:.2}ms rewind={:.2}ms restore={:.2}ms \
                     segments={:.2}ms tail={:.2}ms commit={:.2}ms \
                     submits/event={:.1} submit/event={:.2}ms",
                    to_ms(p.total_phase_stabilize_us) / n,
                    to_ms(p.total_phase_rewind_us) / n,
                    to_ms(p.total_phase_restore_us) / n,
                    to_ms(p.total_phase_segments_us) / n,
                    to_ms(p.total_phase_tail_us) / n,
                    to_ms(p.total_phase_commit_us) / n,
                    p.total_submits as f32 / n,
                    to_ms(p.total_submit_us) / n,
                );
                // Per-dab breakdown — host wall-clock attribution.
                //
                // Top-level split: total = graph_eval + execute_gpu +
                // release_all + flush_submit + post_dab + bookkeeping
                // The first "unattributed" number is whatever escapes
                // even these wider buckets — likely the tiny gaps in
                // `place_dab` between the timed sections.
                //
                // Inside execute_gpu, the per-node buckets (stamp,
                // composite, read_mirror) are subsets. Their delta is
                // framework overhead + other GPU nodes' evaluators.
                let total_dabs = p.total_dabs.max(1) as f32;
                let top_level_attributed = p.total_dab_graph_eval_us
                    + p.total_dab_execute_gpu_us
                    + p.total_dab_release_all_us
                    + p.total_dab_flush_us
                    + p.total_dab_post_us;
                let unattributed = p.total_dab_us.saturating_sub(top_level_attributed);
                let exec_gpu_attributed = p.total_dab_stamp_us
                    + p.total_dab_composite_us
                    + p.total_dab_read_mirror_us
                    + p.total_dab_pool_acquire_us;
                let exec_gpu_other = p
                    .total_dab_execute_gpu_us
                    .saturating_sub(exec_gpu_attributed);
                log::info!(
                    "[stab-perf] per dab top-level (avg µs): \
                     total={:.0} graph_eval={:.0} execute_gpu={:.0} \
                     release_all={:.0} flush_submit={:.0} post_dab={:.0} \
                     unattributed={:.0}",
                    p.total_dab_us as f32 / total_dabs,
                    p.total_dab_graph_eval_us as f32 / total_dabs,
                    p.total_dab_execute_gpu_us as f32 / total_dabs,
                    p.total_dab_release_all_us as f32 / total_dabs,
                    p.total_dab_flush_us as f32 / total_dabs,
                    p.total_dab_post_us as f32 / total_dabs,
                    unattributed as f32 / total_dabs,
                );
                log::info!(
                    "[stab-perf] execute_gpu breakdown (avg µs): \
                     stamp_pass={:.0} composite_pass={:.0} \
                     read_mirror_copy={:.0} pool_acquire={:.0} other={:.0}",
                    p.total_dab_stamp_us as f32 / total_dabs,
                    p.total_dab_composite_us as f32 / total_dabs,
                    p.total_dab_read_mirror_us as f32 / total_dabs,
                    p.total_dab_pool_acquire_us as f32 / total_dabs,
                    exec_gpu_other as f32 / total_dabs,
                );
                // Brush-graph runner breakdown. `steps/dab` is how many
                // GPU-tagged plan steps the runner walks per dab; the two
                // time figures are the per-dab averages of `gather_inputs`
                // and the per-step output write-back loop (both are
                // per-step costs × steps/dab).
                let steps_per_dab = p.total_gpu_steps as f32 / total_dabs;
                // Per-dab time spent in evaluator bodies vs runner framework:
                //   eval_gpu_call: sum of `evaluator.evaluate_gpu(...)` calls
                //   eval_cpu_in_gpu: sum of promoted-CPU `evaluate_cpu(...)` calls
                //   framework = execute_gpu − eval_gpu_call − eval_cpu_in_gpu
                //               − gather_inputs − step_outputs
                let eval_gpu_call_us = p.total_evaluate_gpu_call_us as f32 / total_dabs;
                let eval_cpu_in_gpu_us = p.total_evaluate_cpu_in_gpu_us as f32 / total_dabs;
                let gather_us = p.total_gather_inputs_us as f32 / total_dabs;
                let outputs_us = p.total_step_outputs_us as f32 / total_dabs;
                let exec_gpu_us = p.total_dab_execute_gpu_us as f32 / total_dabs;
                let framework_us =
                    (exec_gpu_us - eval_gpu_call_us - eval_cpu_in_gpu_us - gather_us - outputs_us)
                        .max(0.0);
                log::info!(
                    "[stab-perf] runner per dab: steps/dab={:.1} \
                     gather_inputs={:.0}µs step_outputs={:.0}µs \
                     eval_gpu_call={:.0}µs eval_cpu_in_gpu={:.0}µs \
                     framework={:.0}µs",
                    steps_per_dab,
                    gather_us,
                    outputs_us,
                    eval_gpu_call_us,
                    eval_cpu_in_gpu_us,
                    framework_us,
                );
                // Evaluator-body hotspots — what fills the eval_gpu_call
                // total beyond the bucketed render passes. `prep_no_copy`
                // is the footprint math, isolated from the read_mirror
                // copy already shown above.
                let prep_total = p.total_prepare_canvas_copy_us as f32 / total_dabs;
                let prep_no_copy = (p.total_prepare_canvas_copy_us as i64
                    - p.total_dab_read_mirror_us as i64)
                    .max(0) as f32
                    / total_dabs;
                log::info!(
                    "[stab-perf] evaluator hotspots (avg µs): \
                     prepare_canvas_copy={:.0} (footprint_math={:.0}) \
                     write_composite_uniforms={:.0} \
                     write_stamp_uniforms={:.0} ctx_input={:.0}",
                    prep_total,
                    prep_no_copy,
                    p.total_write_composite_uniforms_us as f32 / total_dabs,
                    p.total_write_stamp_uniforms_us as f32 / total_dabs,
                    p.total_ctx_input_us as f32 / total_dabs,
                );
                if p.full_rerender_events > 0 {
                    log::warn!(
                        "[stab-perf] stroke had {} mid-stroke full re-render \
                         fallback(s) — checkpoint ring lost coverage of the \
                         divergence window. See docs/brush/stabilization.md \
                         §\"Checkpoint Ring Invariants\".",
                        p.full_rerender_events,
                    );
                }
            }

            // Per-stroke thumbnail refresh — the node texture (raster or mask
            // modifier) now holds the cumulative pixels of every dab/op since
            // begin_stroke. Live mid-stroke updates are intentionally skipped.
            self.compositor.mark_node_pixels_dirty(layer_id);

            // If a flood fill is pending, defer undo commit — complete_flood_fill
            // will handle it when the readback arrives.
            if self
                .readbacks
                .any(|c| matches!(c, ReadbackContext::FloodFill { .. }))
            {
                return;
            }

            // Finalize brush stroke engine and destroy stroke buffer + checkpoints.
            if let Some(engine) = self.brush_stroke_engine.take() {
                let _record = engine.end();
            }
            self.stroke_buffer = None;
            self.checkpoint_ring.clear();

            // Dispatch GPU diff to find the exact changed region for undo.
            if let (Some(snap), true) = (
                self.scratch_snapshot.take(),
                self.pending_undo_commit.is_none(),
            ) {
                let layer_extent = self
                    .compositor
                    .node_texture(layer_id)
                    .map(|t| (&t.view, t.canvas_extent()));
                if let Some((current_view, layer_canvas_extent)) = layer_extent {
                    let scratch_view = self.region_store.scratch_view(snap.format);
                    self.diff_rect.request(
                        &self.gpu.device,
                        &self.gpu.queue,
                        &scratch_view,
                        current_view,
                        layer_canvas_extent,
                    );
                    self.pending_undo_commit = Some(PendingUndoCommit {
                        layer_id,
                        snapshot: snap,
                    });
                }
            }
        }
    }

    // --- GPU erase helpers ---

    /// Clear layer pixels within the current selection via GPU erase pass.
    pub(crate) fn gpu_clear_selection(&mut self, layer_id: LayerId) {
        if !self.has_selection() {
            return;
        }

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let format = match self.paint_target(layer_id) {
            Some(t) => t.format,
            None => return,
        };
        let rect = crate::coord::CanvasRect::from_xywh(0, 0, canvas_w, canvas_h);

        // Inline dispatch helper for use INSIDE the gpu.encode closures.
        // `paint_target()` is a method call which the closure-capture
        // analyser can't split-borrow through, so closures need direct
        // field access (`self.compositor.X`) to compile alongside
        // `self.region_store` / `self.undo_stack` access.
        macro_rules! pt_for {
            () => {
                GpuPaintTarget::from_node(
                    self.compositor.node_texture(layer_id).unwrap(),
                    canvas_w,
                    canvas_h,
                )
            };
        }

        // Save region for undo.
        let snap = self.gpu.encode_ret("clear-sel-save", |encoder| {
            let frame = pt_for!().canvas_frame();
            self.region_store
                .save_region(&self.gpu.device, encoder, &frame, format, rect)
        });

        // Erase within selection using the cached GPU selection bind group.
        let sel_bg = self
            .compositor
            .selection_state()
            .map(|s| s.paint_bind_group())
            .expect("has_selection true → selection_state allocated");
        self.gpu.encode("clear-sel-erase", |encoder| {
            pt_for!().erase_with_selection(encoder, &self.paint_pipelines, &self.gpu.queue, sel_bg);
        });

        // Commit for undo.
        self.gpu.encode("clear-sel-commit", |encoder| {
            let frame = pt_for!().canvas_frame();
            let entry = self
                .region_store
                .commit_region(encoder, layer_id, &frame, &snap, rect);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
        self.compositor.mark_node_pixels_dirty(layer_id);
    }

    /// Clear entire layer to transparent via GPU.
    pub(crate) fn gpu_clear_layer(&mut self, layer_id: LayerId) {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let format = match self.paint_target(layer_id) {
            Some(t) => t.format,
            None => return,
        };
        let rect = crate::coord::CanvasRect::from_xywh(0, 0, canvas_w, canvas_h);

        // Inline dispatch helper — see `gpu_clear_selection` for why a macro
        // is needed instead of calling `self.paint_target(...)` directly.
        macro_rules! pt_for {
            () => {
                GpuPaintTarget::from_node(
                    self.compositor.node_texture(layer_id).unwrap(),
                    canvas_w,
                    canvas_h,
                )
            };
        }

        // Save region for undo.
        let snap = self.gpu.encode_ret("clear-layer-save", |encoder| {
            let frame = pt_for!().canvas_frame();
            self.region_store
                .save_region(&self.gpu.device, encoder, &frame, format, rect)
        });

        // Clear the full canvas.
        self.gpu.encode("clear-layer", |encoder| {
            pt_for!().clear_rect(
                encoder,
                &self.paint_pipelines,
                &self.gpu.queue,
                [0, 0, canvas_w as i32, canvas_h as i32],
            );
        });

        // Commit for undo.
        self.gpu.encode("clear-layer-commit", |encoder| {
            let frame = pt_for!().canvas_frame();
            let entry = self
                .region_store
                .commit_region(encoder, layer_id, &frame, &snap, rect);
            self.undo_stack.push(Box::new(GpuRegionAction::new(entry)));
        });
        self.compositor.mark_node_pixels_dirty(layer_id);
    }

    /// Resolve the active paint target for a layer.
    ///
    /// Resolve the GPU paint target for a node id. Format-driven dispatch
    /// (R8 mask vs RGBA layer) lives behind the unified node-texture pool —
    /// callers don't branch on the kind. Returns `None` for groups, unknown
    /// ids, or any node without a `PixelBuffer`.
    pub(crate) fn paint_target(&self, node_id: LayerId) -> Option<GpuPaintTarget<'_>> {
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        self.compositor
            .node_texture(node_id)
            .map(|t| GpuPaintTarget::from_node(t, canvas_w, canvas_h))
    }

    /// Upload a cropped region of the GPU selection as an R8 texture bind group.
    /// Reads from the CPU cache (populated by async readback or eagerly on upload).
    pub(crate) fn upload_cropped_selection_r8(
        &self,
        origin: (i32, i32),
        width: u32,
        height: u32,
    ) -> Option<wgpu::BindGroup> {
        if !self.has_selection() {
            return None;
        }

        let full = self.selection_cpu_cache()?;
        let (ox, oy) = origin;
        let cw = self.doc.width;
        let ch = self.doc.height;

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
