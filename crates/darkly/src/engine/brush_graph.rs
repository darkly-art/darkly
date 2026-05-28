//! Brush graph management methods on DarklyEngine.
//!
//! Provides the API surface for the WASM bridge to query node types,
//! get/set the active brush graph, and compile graphs.

use super::{DarklyEngine, ReadbackContext};
use crate::brush::state::BrushState;
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::gpu::params::ParamValue;
use crate::nodegraph::Graph;
use crate::nodegraph::{NodeId, PortDir, PortRef, UnitType};

/// Panic message used by every `tool_session` BrushState lookup. The
/// session is seeded with `BrushState::new()` at engine construction;
/// `None` here would mean someone removed the entry, which is a bug.
const NO_BRUSH_STATE: &str = "BrushState registered at session init";

/// Classifies a brush-graph mutation by which preview consumers it
/// actually invalidates.
#[derive(Copy, Clone)]
enum ChangeKind {
    /// Structural or non-scrub change: nodes, wires, params, exposed
    /// flags, non-exposed port defaults, brush load/reset/clear. Bumps
    /// both `brush_graph_version` and `brush_topology_version`.
    Topology,
    /// Exposed-port scrub on a port marked `persist_in_thumbnail` — its
    /// value bleeds through to the dab thumbnail render, so both
    /// version counters need to bump to invalidate both preview caches.
    /// Used for orientation knobs like `stamp.rotation`.
    ThumbnailRelevantScrub,
    /// User-facing exposed-port scrub on a port the editor preview
    /// pipeline actually reads (size, opacity, hardness, …). Bumps only
    /// `brush_graph_version` — the dab thumbnail render neutralises
    /// scrubs via `reset_exposed_scrubs`, so its cache stays valid.
    ScrubOnly,
    /// Exposed-port scrub on a port the editor preview pipeline
    /// ignores — declared via `PortDef::preview_value`, applied by
    /// `Graph::apply_preview_overrides` before the preview renders. The
    /// rendered output cannot change, so neither cache needs to bump.
    /// Used for `pen_input.stabilize`, `pen_input.spacing`, and
    /// `stamp.size` (preview overrides them all to fixed values).
    PreviewIrrelevantScrub,
}

impl DarklyEngine {
    /// Return metadata for all registered brush node types.
    ///
    /// Returns the bare nodegraph registration (ports, params, display
    /// info) — the wrapper's pipeline metadata is engine-internal and the
    /// frontend doesn't see it.
    pub fn brush_node_types(&self) -> Vec<crate::nodegraph::NodeRegistration<BrushWireType>> {
        let registry = BrushNodeRegistry::new();
        registry.types().map(|r| r.node.clone()).collect()
    }

    /// Does the active brush graph's terminal honor erase mode?
    ///
    /// True iff every terminal node in the active graph's registration
    /// has `supports_erase = true`. Type-owned dispatch — there is no
    /// central list of which terminals don't (smudge, liquify,
    /// watercolor today); each module's `register()` declares its own
    /// value.
    ///
    /// Used by the brush-tool options bar to hide the erase button for
    /// terminals where flipping `gpu.blend_mode` would do nothing.
    pub fn active_brush_supports_erase(&self) -> bool {
        let graph = self.active_brush_graph();
        let registry = BrushNodeRegistry::new();
        for node in graph.nodes.values() {
            let Some(reg) = registry.get(&node.type_id) else {
                continue;
            };
            if reg.node.is_terminal && !reg.node.supports_erase {
                return false;
            }
        }
        // No terminal, or every terminal supports erase → keep the
        // toggle visible.
        true
    }

    /// Return a clone of the default brush graph.
    pub fn default_brush_graph(&self) -> Graph<BrushWireType> {
        crate::brush::default_graph()
    }

    // `active_brush_graph()` and `brush_graph_version()` /
    // `brush_topology_version()` live on `super::DarklyEngine` (see
    // `engine/mod.rs`) — they pull from the shared brush session under
    // a read lock. Same public API, different storage.

    /// Validate a brush graph from JSON without setting it as active.
    ///
    /// Returns `Ok(())` or an error string describing what's wrong.
    pub fn validate_brush_graph(&self, json: &str) -> Result<(), String> {
        crate::brush::validate_graph_json(json)
    }

    /// Compile a brush graph from JSON and set it as the active graph.
    ///
    /// The next stroke will use this graph.  Returns `Ok(())` on success
    /// or an error string if the graph is invalid.
    pub fn set_brush_graph(&mut self, json: &str) -> Result<(), String> {
        // Validate by attempting compilation.
        let _runner = crate::brush::compile_from_json(json)?;
        // If compilation succeeded, store the deserialized graph.
        let graph: Graph<BrushWireType> =
            serde_json::from_str(json).map_err(|e| format!("JSON parse error: {e}"))?;
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph = graph;
        self.snapshot_brush_defaults();
        // Run the post-mutation pipeline so the brush preview mask (and any
        // other graph-dependent state) refreshes from the new graph.
        self.compile_active(ChangeKind::Topology)?;
        Ok(())
    }

    /// Reset the active brush graph to the built-in default.
    pub fn reset_brush_graph(&mut self) {
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph = crate::brush::default_graph();
        self.snapshot_brush_defaults();
        let _ = self.compile_active(ChangeKind::Topology);
    }

    /// Capture every input port's current default into the shared brush
    /// state's `defaults` map. Called whenever the active graph is
    /// replaced as a whole — brush load, reset, save — so that "reset to
    /// default" returns to the loaded/saved baseline rather than the
    /// node-type registration value. Not called on individual port edits;
    /// that's the whole point.
    pub(crate) fn snapshot_brush_defaults(&mut self) {
        let mut tool = self.tool_session.write();
        let brush = tool.get_mut::<BrushState>().expect(NO_BRUSH_STATE);
        brush.defaults.clear();
        // Re-borrow split: walking `brush.graph.nodes` and inserting
        // into `brush.defaults` are reads/writes of disjoint fields on
        // `BrushState`, so the borrow checker permits both inside the
        // loop with no separate snapshot.
        for node in brush.graph.nodes.values() {
            for port in &node.ports {
                if port.dir == PortDir::Input {
                    brush
                        .defaults
                        .insert((node.id, port.name.clone()), port.default);
                }
            }
        }
    }

    // --- Fine-grained graph commands ---

    /// Re-render the brush preview into the overlay's preview mask using
    /// fully-synthetic pen inputs. Fired on graph/param changes where no
    /// real pen data is available — clears any hover history so the next
    /// hover starts fresh (no bogus direction carried across a brush
    /// swap, etc.).
    pub fn regenerate_brush_preview(&mut self) {
        self.last_preview_pose = None;
        let dummy = crate::brush::paint_info::PaintInformation::preview_dummy();
        self.regenerate_brush_preview_with_pen_internal(dummy);
    }

    /// Drop the remembered hover pose so the next
    /// `regenerate_brush_preview_with_pen` starts a fresh hover with no
    /// derived direction/motion/distance/speed. Call this on pointer-leave
    /// and at the start of a stroke.
    pub fn clear_brush_preview_pose(&mut self) {
        self.last_preview_pose = None;
    }

    /// Re-render the brush preview using live hover data.
    ///
    /// Pre-fills `pen`'s segment-derived sensors (drawing_angle, motion,
    /// distance, speed) using the previous hover pose — the same helper
    /// the stroke engine uses — so a compiled graph wiring any sensor
    /// into any input sees the same values the upcoming stroke would.
    ///
    /// The rest of `pen` (pos, pressure, tilts, rotation,
    /// tangential_pressure, time) comes from the PointerEvent; tilt
    /// magnitude/direction are derived from the reported tilts. The pose
    /// is stored for the next call's derivation.
    pub fn regenerate_brush_preview_with_pen(
        &mut self,
        mut pen: crate::brush::paint_info::PaintInformation,
    ) {
        // Chord length between the previous and current hover positions.
        // Chord rather than Catmull-Rom arc length — there is no spline
        // through a single sample.
        let segment_length = match &self.last_preview_pose {
            Some(prev) => {
                let dx = pen.pos[0] - prev.pos[0];
                let dy = pen.pos[1] - prev.pos[1];
                (dx * dx + dy * dy).sqrt()
            }
            None => 0.0,
        };
        pen.derive_sensors(self.last_preview_pose.as_ref(), segment_length);
        self.last_preview_pose = Some(pen);
        self.regenerate_brush_preview_with_pen_internal(pen);
    }

    /// Shared render body — no pose tracking, no sensor derivation.
    /// `pen` must already be fully populated by the caller.
    fn regenerate_brush_preview_with_pen_internal(
        &mut self,
        pen: crate::brush::paint_info::PaintInformation,
    ) {
        use crate::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};

        // Compile under a read guard; drop the guard before any GPU
        // work to keep the critical section narrow. The guard is held
        // only for the synchronous compile_graph call.
        let mut runner = {
            let tool = self.tool_session.read();
            let brush = tool.get::<BrushState>().expect(NO_BRUSH_STATE);
            match crate::brush::compile_graph(&brush.graph) {
                Ok(r) => r,
                Err(_) => {
                    drop(tool);
                    self.compositor.clear_overlay_preview_mask();
                    self.brush_preview_info = None;
                    return;
                }
            }
        };

        // Always dispatch `render_preview` — individual terminals decide
        // whether they produce output this frame. A graph with no
        // compiled-terminal hook fires nothing and `brush_preview_info`
        // stays None; the four compiled terminals each fire their
        // hook and publish placement info. The post-run
        // `info.is_some()` check below routes both outcomes.

        // Split-borrow the compositor so we can hold a mutable handle
        // on `tool_overlay` (for the terminal's `ensure_preview_mask`
        // grow) alongside an immutable borrow of `selection_state` for
        // the brush bind group. The two fields are disjoint;
        // `Compositor::split_overlay_and_selection` documents the
        // pattern.
        let (overlay, selection) = self.compositor.split_overlay_and_selection();
        let has_selection = selection.is_some();
        let sel_bg = if has_selection {
            selection
                .map(|s| s.brush_bind_group())
                .unwrap_or(&self.brush_pipelines.default_selection_bind_group)
        } else {
            &self.brush_pipelines.default_selection_bind_group
        };
        let encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("brush-preview-regen"),
            });

        let mut gpu_ctx = BrushGpuContext {
            encoder,
            device: &self.gpu.device,
            queue: &self.gpu.queue,
            pipelines: &self.brush_pipelines,
            // The preview pipeline doesn't touch the stroke scratch — the
            // terminal's `render_preview` writes to the preview mask
            // through `preview_mask_overlay` instead. No `Scratch` is
            // needed; any accidental call to a scratch accessor will
            // panic, exposing the bug.
            scratch: None,
            canvas_width: 0,
            canvas_height: 0,
            // No layer / pre-stroke state in preview — commit isn't called,
            // and `render_preview` writes to the preview mask.
            paint_target: None,
            selection_bind_group: sel_bg,
            preview_target_view: None,
            blend_mode: 0,
            // Tests pre-allocate `preview_mask_view`; the engine path
            // grows the mask on demand via `preview_mask_overlay`.
            preview_mask_view: None,
            preview_mask_size: (0, 0),
            preview_mask_overlay: Some(overlay),
            brush_preview_info: None,
            pre_stroke_texture: None,
            pre_stroke_bind_group: None,
            dab_write_canvas_bbox: None,
            perf: BrushPerfCounters::default(),
            pending_dab_bytes: Vec::new(),
            pending_dab_count: 0,
            pending_dabs_bbox: None,
            pending_dab_meta_bytes: Vec::new(),
            compiled_brush: None,
            slot_outputs_owned: None,
        };

        self.brush_pipelines.reset_uniform_rings();
        runner.clear_slots();
        runner.seed_sensors(&pen, [1.0, 1.0, 1.0, 1.0], 0, 0);
        runner.execute_cpu();
        runner.render_preview_pipeline(&mut gpu_ctx);

        let info = gpu_ctx.brush_preview_info;
        let command_buf = gpu_ctx.encoder.finish();
        self.gpu.queue.submit([command_buf]);

        if info.is_some() {
            self.compositor.use_overlay_preview_mask();
        } else {
            self.compositor.clear_overlay_preview_mask();
        }
        self.brush_preview_info = info;
    }

    /// Read-only snapshot of the current brush preview info, for the
    /// frontend to place the hover overlay primitive.
    pub fn brush_preview_info(&self) -> Option<crate::brush::eval::BrushPreviewInfo> {
        self.brush_preview_info
    }

    /// Compile the active graph in-place.
    ///
    /// `kind` selects which version counters to bump:
    /// - [`ChangeKind::Topology`] bumps both the graph version (editor /
    ///   hover preview) and the topology version (dab thumbnail).
    /// - [`ChangeKind::ScrubOnly`] bumps only the graph version. The dab
    ///   thumbnail render neutralises exposed-port scrubs via
    ///   [`crate::brush::reset_exposed_scrubs`], so a scrub change can't
    ///   change its rendered output — no point invalidating its cache.
    /// - [`ChangeKind::PreviewIrrelevantScrub`] bumps neither. The
    ///   scrubbed port is overridden by
    ///   [`crate::nodegraph::Graph::apply_preview_overrides`] before
    ///   every editor-preview render, so its rendered output is
    ///   independent of the user's port value — invalidating the cache
    ///   would just cause a wasted full-stroke re-render.
    ///
    /// Returns Ok on success or an error string.
    fn compile_active(&mut self, kind: ChangeKind) -> Result<(), String> {
        {
            let tool = self.tool_session.read();
            let brush = tool.get::<BrushState>().expect(NO_BRUSH_STATE);
            crate::brush::compile_graph(&brush.graph).map_err(|e| format!("{e}"))?;
        }

        // Bump version counters per the change classification — see the
        // `ChangeKind` doc above for the full rule. PreviewIrrelevantScrub
        // bumps nothing: the rendered preview output can't have changed.
        match kind {
            ChangeKind::Topology | ChangeKind::ThumbnailRelevantScrub => {
                self.bump_brush_topology_version()
            }
            ChangeKind::ScrubOnly => self.bump_brush_graph_version(),
            ChangeKind::PreviewIrrelevantScrub => {}
        }

        // Refresh the brush preview overlay now that the graph is compiled —
        // size, rotation, and tip changes all land here.
        self.regenerate_brush_preview();

        Ok(())
    }

    /// Set the theme colors used by the editor live preview and by
    /// brush thumbnail baking. Both paths share one palette so the live
    /// preview visually matches the brush picker's grid thumbnails.
    ///
    /// Invalidates the cached editor preview, the active-dab preview,
    /// and every per-brush PNG thumbnail in the library so the next
    /// picker refresh re-bakes against the new palette.
    pub fn set_preview_theme(&mut self, fg: [f32; 4], bg: [f32; 4]) {
        if self.preview_theme_fg == fg && self.preview_theme_bg == bg {
            return;
        }
        self.preview_theme_fg = fg;
        self.preview_theme_bg = bg;
        self.invalidate_brush_editor_preview();
        // Drop baked PNG thumbnails so picker tiles re-bake on demand.
        // The frontend's rAF poll handles the empty→bake→present flow.
        self.brush_library.clear_thumbnails();
    }

    /// Render a full-stroke brush editor preview and return the most recent
    /// cached PNG bytes synchronously. The pixels update on a later frame
    /// once the async readback completes — same shape as
    /// `brush_active_dab_preview`. Always framed to `BRUSH_THUMBNAIL_SIZE`;
    /// the frontend scales the result via CSS to whatever display size it
    /// needs.
    ///
    /// Uses the theme colors stored via `set_preview_theme`, not the user's
    /// active paint color — keeps the editor preview visually consistent
    /// with the brush picker's brush thumbnails.
    pub fn brush_editor_preview(&mut self) -> Vec<u8> {
        // Guard against painting while a real stroke is in flight — the
        // preview shares `dab_pool` and `brush_pipelines` with the engine,
        // and running mid-stroke would step on acquired handles and
        // uniform rings.
        let in_stroke = self.brush_stroke_engine.is_some();

        // Caller's frontend treats an empty Vec as "no fresh bytes
        // available" and skips the image update — preserving whatever was
        // last shown. A zero-filled buffer would *also* parse cleanly and
        // render as a transparent image, wiping the visible preview.
        let cached = self.brush_editor_preview_cache.clone();

        // Skip work when nothing has changed and the cache is good. Also
        // skip if a real stroke is in progress — return the most recent
        // cached bytes so the UI stays responsive without clobbering the
        // stroke's GPU state.
        let current_graph_version = self.brush_graph_version();
        let nothing_to_do = in_stroke
            || (self.last_rendered_preview_version == current_graph_version
                && self.brush_editor_preview_cache.is_some());
        if nothing_to_do {
            return cached.unwrap_or_default();
        }

        // Don't queue a second readback on top of an in-flight one — it
        // would race with whichever lands first and the stale result
        // could overwrite the fresh one.
        let already_pending = self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::BrushEditorPreview { .. }));
        if already_pending {
            return cached.unwrap_or_default();
        }

        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;

        // Clone the active graph and neutralize any ports flagged with
        // `preview_max` so the rendered stroke fits the fixed render
        // canvas regardless of the user's working brush parameters.
        // Per-node knowledge about what to neutralize lives on the port
        // registrations; this pipeline doesn't introspect node types.
        let mut graph = self.active_brush_graph();
        graph.apply_preview_overrides();
        let (rw, rh) = super::brush_library::BRUSH_STROKE_RENDER_SIZE;
        let path = crate::brush::preview_renderer::synthesize_preview_stroke(
            rw as f32,
            rh as f32,
            30,
            super::brush_library::BRUSH_STROKE_PATH_INSET,
        );
        self.render_preview_and_request_readback(
            &graph,
            &path,
            rw,
            rh,
            fg,
            bg,
            ReadbackContext::BrushEditorPreview {
                width: rw,
                height: rh,
                graph_version: current_graph_version,
            },
        );
        self.last_rendered_preview_version = current_graph_version;

        cached.unwrap_or_default()
    }

    /// Invalidate any cached editor preview — call when the theme colors
    /// change so the next `brush_editor_preview` request re-renders with
    /// the new palette instead of returning the stale cached pixels.
    /// Also drops the active-dab preview cache so the BrushBar trigger
    /// thumbnail and the picker's active-brush strip refresh on the same
    /// signal.
    pub fn invalidate_brush_editor_preview(&mut self) {
        self.brush_editor_preview_cache = None;
        self.active_dab_preview_cache = None;
        // Theme changes alter rendered colors → both editor preview and
        // dab thumbnail need to re-render and discard any in-flight
        // readbacks. Bump both versions.
        self.bump_brush_topology_version();
    }

    /// Render a single-dab preview of the active brush and return the
    /// most recent cached PNG bytes synchronously. Pixels update on a
    /// later frame once the async readback completes — same shape as
    /// `brush_editor_preview` and `layer_thumbnail`. Used by the
    /// BrushBar trigger button and the picker's active-brush strip.
    ///
    /// Renders at the same fixed `BRUSH_DAB_RENDER_SIZE` the baked
    /// thumbnail path uses, and runs the result through the same
    /// `frame_dab_thumbnail` framer — so the bytes returned here are
    /// byte-identical to a `brush_dab_thumbnail(active_name)` call.
    /// The frontend scales the resulting PNG via CSS to whatever
    /// display size it needs.
    pub fn brush_active_dab_preview(&mut self) -> Vec<u8> {
        // Guard against painting while a real stroke is in flight — the
        // preview shares `dab_pool` and `brush_pipelines` with the engine,
        // and running mid-stroke would step on acquired handles and
        // uniform rings.
        let in_stroke = self.brush_stroke_engine.is_some();

        // See `brush_editor_preview` for why we return an empty Vec rather
        // than a zero-filled one when no cache is available — frontends
        // treat empty as "no fresh bytes" and preserve the last successful
        // render, while a zero buffer would parse as a transparent image
        // and visibly wipe whatever was on screen.
        let cached = self.active_dab_preview_cache.clone();

        // Skip work when nothing has changed and the cache is good. Also
        // skip while a real stroke is in progress — return the most recent
        // cached bytes so the UI stays responsive without clobbering the
        // stroke's GPU state.
        let current_topology = self.brush_topology_version();
        let nothing_to_do = in_stroke
            || (self.last_rendered_dab_topology_version == current_topology
                && self.active_dab_preview_cache.is_some());
        if nothing_to_do {
            return cached.unwrap_or_default();
        }

        // Don't queue a second readback on top of an in-flight one.
        let already_pending = self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::ActiveBrushDab { .. }));
        if already_pending {
            return cached.unwrap_or_default();
        }

        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;
        // Reset every exposed scrub (size, opacity, hardness, …) to its
        // registration default before rendering. The dab thumbnail
        // represents the brush's identity (shape, texture, dynamics);
        // user-facing scrubs belong in the brush bar, not the icon.
        let mut graph = self.active_brush_graph();
        crate::brush::reset_exposed_scrubs(&mut graph);
        let (rw, rh) = super::brush_library::BRUSH_DAB_RENDER_SIZE;
        let path = crate::brush::preview_renderer::synthesize_preview_dab(rw as f32, rh as f32);
        self.render_preview_and_request_readback(
            &graph,
            &path,
            rw,
            rh,
            fg,
            bg,
            ReadbackContext::ActiveBrushDab {
                topology_version: current_topology,
            },
        );
        self.last_rendered_dab_topology_version = current_topology;

        cached.unwrap_or_default()
    }

    /// Per-node thumbnail of a single GPU node's `texture` output.
    ///
    /// Unimplemented — returns the empty Vec ("no fresh bytes yet —
    /// preserve last shown") so frontend node-card thumbnails fall
    /// back to their placeholder state. Brush previews are rendered
    /// per-terminal, not per-node; this entry point remains so the
    /// frontend's node-card API surface stays stable.
    pub fn brush_node_preview(&mut self, _node_id: u64) -> Vec<u8> {
        Vec::new()
    }

    /// Shared helper: render a preview path into the preview renderer's
    /// texture, then encode an async readback tagged with `context`. The
    /// caller decides what to do with the bytes when they arrive. The
    /// graph is taken explicitly so callers can render thumbnails for
    /// library brushes without touching the active graph; the path lets
    /// callers choose between the S-curve stroke and a single-dab preview.
    pub(crate) fn render_preview_and_request_readback(
        &mut self,
        graph: &Graph<BrushWireType>,
        path: &[crate::brush::paint_info::PaintInformation],
        width: u32,
        height: u32,
        fg: [f32; 4],
        bg: [f32; 4],
        context: ReadbackContext,
    ) {
        let Some(texture) = self.brush_preview_renderer.render_stroke(
            &self.gpu.device,
            &self.gpu.queue,
            &self.brush_pipelines,
            graph,
            path,
            fg,
            bg,
            width,
            height,
        ) else {
            return;
        };

        // Encode the readback manually (not via `gpu.encode`) so the
        // borrow of `self.brush_preview_renderer` that produced
        // `texture` coexists with borrows of `self.gpu` and
        // `self.readbacks` — they're disjoint fields of `self`.
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("brush-editor-preview-readback"),
            });
        let request = crate::gpu::readback::request_readback(
            &self.gpu.device,
            &mut encoder,
            texture,
            wgpu::TextureFormat::Rgba8Unorm,
            crate::coord::LayerRect::from_xywh(0, 0, width, height),
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.readbacks.submit(request, context);
    }

    /// Serialize the active graph as JSON.
    fn active_graph_json(&self) -> String {
        let tool = self.tool_session.read();
        let brush = tool.get::<BrushState>().expect(NO_BRUSH_STATE);
        serde_json::to_string(&brush.graph).unwrap_or_else(|_| "null".into())
    }

    /// Add a node to the active graph and compile.
    /// Returns the updated graph JSON on success.
    pub fn brush_graph_add_node(&mut self, type_id: &str) -> Result<String, String> {
        let registry = BrushNodeRegistry::new();
        let reg = registry
            .get(type_id)
            .ok_or_else(|| format!("unknown node type: {type_id}"))?;

        let params = reg
            .params
            .iter()
            .map(|p| p.default_value())
            .collect::<Vec<_>>();
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .add_node(type_id, reg.ports.clone(), params);

        self.compile_active(ChangeKind::Topology)?;
        Ok(self.active_graph_json())
    }

    /// Remove a node from the active graph and compile.
    pub fn brush_graph_remove_node(&mut self, node_id: u64) -> Result<String, String> {
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .remove_node(NodeId(node_id))
            .map_err(|e| format!("{e}"))?;
        self.compile_active(ChangeKind::Topology)?;
        Ok(self.active_graph_json())
    }

    /// Connect two ports in the active graph and compile.
    pub fn brush_graph_connect(
        &mut self,
        from_node: u64,
        from_port: &str,
        to_node: u64,
        to_port: &str,
    ) -> Result<String, String> {
        let to_ref = PortRef {
            node: NodeId(to_node),
            port: to_port.into(),
        };
        {
            let mut tool = self.tool_session.write();
            let brush = tool.get_mut::<BrushState>().expect(NO_BRUSH_STATE);
            // Remove any existing connection to this input first.
            brush.graph.connections.retain(|c| c.to != to_ref);
            brush
                .graph
                .connect(
                    PortRef {
                        node: NodeId(from_node),
                        port: from_port.into(),
                    },
                    to_ref.clone(),
                )
                .map_err(|e| format!("{e}"))?;
        }
        self.compile_active(ChangeKind::Topology)?;
        Ok(self.active_graph_json())
    }

    /// Disconnect a specific wire in the active graph and compile.
    pub fn brush_graph_disconnect(
        &mut self,
        from_node: u64,
        from_port: &str,
        to_node: u64,
        to_port: &str,
    ) -> Result<String, String> {
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .disconnect(
                &PortRef {
                    node: NodeId(from_node),
                    port: from_port.into(),
                },
                &PortRef {
                    node: NodeId(to_node),
                    port: to_port.into(),
                },
            );
        self.compile_active(ChangeKind::Topology)?;
        Ok(self.active_graph_json())
    }

    /// Update a parameter on a node and compile.
    pub fn brush_graph_set_param(
        &mut self,
        node_id: u64,
        param_index: usize,
        value: ParamValue,
    ) -> Result<String, String> {
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .set_param(NodeId(node_id), param_index, value)
            .map_err(|e| format!("{e}"))?;
        self.compile_active(ChangeKind::Topology)?;
        Ok(self.active_graph_json())
    }

    /// Update a port's default value and compile.
    pub fn brush_graph_set_port_default(
        &mut self,
        node_id: u64,
        port_name: &str,
        value: f32,
    ) -> Result<String, String> {
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .set_port_default(NodeId(node_id), port_name, value)
            .map_err(|e| format!("{e}"))?;
        self.compile_active(ChangeKind::Topology)?;
        Ok(self.active_graph_json())
    }

    /// Compute auto-layout positions for the active brush graph.
    /// `sizes` maps `NodeId` → `[width, height]` measured from the DOM.
    /// Returns the layout map directly — positions are a UI-only concern
    /// and are not stored on the graph.
    pub fn brush_graph_auto_layout(
        &self,
        sizes: &std::collections::HashMap<NodeId, [f32; 2]>,
    ) -> crate::nodegraph::NodeLayout {
        self.tool_session
            .read()
            .get::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .auto_layout_with_sizes(sizes)
    }

    /// Upload an RGBA8 image and associate it with a resource name.
    ///
    /// Image-stamp brushes are unsupported — `stamp` only accepts
    /// AlphaMask application, which compiles inline without sampling
    /// an RGBA tip texture. The entry point remains so the frontend's
    /// upload UI doesn't fault on a missing symbol; it returns an
    /// error rather than silently dropping the bytes.
    pub fn brush_upload_image(
        &mut self,
        _resource_name: &str,
        _width: u32,
        _height: u32,
        _rgba: &[u8],
    ) -> Result<(), String> {
        Err("image-stamp brushes are unsupported — stamp accepts \
             AlphaMask only"
            .to_string())
    }

    /// Set the composite blend mode: 0 = source-over (paint), 1 = destination-out (erase).
    pub fn set_brush_blend_mode(&mut self, mode: u32) {
        self.brush_blend_mode = mode;
    }

    /// Return info about all exposed ports in the active brush graph.
    ///
    /// Scans all nodes for input ports with `exposed == true`.
    ///
    /// The result is ordered by auto-layout position (top-to-bottom,
    /// left-to-right) for a stable order in the properties panel.
    pub fn brush_exposed_ports(&self) -> Vec<ExposedPortInfo> {
        let registry = BrushNodeRegistry::new();
        let tool = self.tool_session.read();
        let brush = tool.get::<BrushState>().expect(NO_BRUSH_STATE);
        let layout = brush.graph.auto_layout();
        let mut result: Vec<ExposedPortInfo> = Vec::new();

        for node in brush.graph.nodes.values() {
            let reg = registry.get(&node.type_id);
            let display_name = reg.map(|r| r.display_name).unwrap_or("");

            for port in &node.ports {
                if !port.exposed || port.dir != PortDir::Input {
                    continue;
                }

                // Only Scalar ports for now.
                if port.wire_type != BrushWireType::Scalar {
                    continue;
                }

                // A connected input is driven by its wire, not the user.
                let connected = brush
                    .graph
                    .connections
                    .iter()
                    .any(|c| c.to.node == node.id && c.to.port == port.name);
                if connected {
                    continue;
                }

                // Display metadata comes from the registration (canonical),
                // per-instance state (default, exposed) from the instance.
                let reg_port = reg.and_then(|r| {
                    r.ports
                        .iter()
                        .find(|rp| rp.name == port.name && rp.dir == port.dir)
                });
                let unit_type = reg_port.map_or(port.unit_type, |rp| rp.unit_type);
                let label = reg_port
                    .map(|rp| &rp.label)
                    .filter(|l| !l.is_empty())
                    .cloned()
                    .unwrap_or_else(|| port.name.clone());
                let icon = reg_port.map_or_else(|| port.icon.clone(), |rp| rp.icon.clone());
                let description =
                    reg_port.map_or_else(|| port.description.clone(), |rp| rp.description.clone());

                // Reset target = the value snapshotted at brush load
                // time. Falls back to the registration default for ports
                // on nodes the user added after load (those weren't part
                // of the brush, so registration default is the right
                // baseline).
                let reset_default = brush
                    .defaults
                    .get(&(node.id, port.name.clone()))
                    .copied()
                    .unwrap_or_else(|| reg_port.map(|rp| rp.default).unwrap_or(port.default));
                result.push(ExposedPortInfo {
                    node_id: node.id.0,
                    port_name: port.name.clone(),
                    label,
                    icon,
                    description,
                    node_display_name: display_name.to_string(),
                    data: ExposedValue::Scalar {
                        value: unit_type.to_display(port.default),
                        min: unit_type.to_display(port.min),
                        max: unit_type.to_display(port.max),
                        default: unit_type.to_display(reset_default),
                        unit_type,
                    },
                });
            }
        }

        // Sort by layout position: top-to-bottom (y), then left-to-right (x).
        // Layout is computed above; entries for unknown nodes default to origin.
        let key = |info: &ExposedPortInfo| -> [f32; 2] {
            layout
                .get(&NodeId(info.node_id))
                .copied()
                .unwrap_or([0.0, 0.0])
        };
        result.sort_by(|a, b| {
            let ka = key(a);
            let kb = key(b);
            ka[1]
                .partial_cmp(&kb[1])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    ka[0]
                        .partial_cmp(&kb[0])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        result
    }

    /// Set an exposed port's value from display-space, converting to
    /// port-space via the port's UnitType.  Compiles afterward.
    pub fn brush_set_exposed_port(
        &mut self,
        node_id: u64,
        port_name: &str,
        display_value: f32,
    ) -> Result<String, String> {
        let nid = NodeId(node_id);

        // Snapshot the node's type_id under a brief read guard so the
        // registry lookup below doesn't have to re-acquire the lock.
        // All later mutation happens through a fresh write guard.
        let type_id = {
            let tool = self.tool_session.read();
            let brush = tool.get::<BrushState>().expect(NO_BRUSH_STATE);
            match brush.graph.nodes.get(&nid) {
                Some(node) => node.type_id.clone(),
                None => return Err(format!("node {node_id} not found")),
            }
        };

        // Look up UnitType + preview_value + persist_in_thumbnail from
        // the registration. One port lookup pays for all three flags;
        // they determine whether this scrub affects the editor preview
        // and/or the dab thumbnail (see `ChangeKind` docs).
        let registry = BrushNodeRegistry::new();
        let port_meta = registry.get(&type_id).and_then(|r| {
            r.ports
                .iter()
                .find(|rp| rp.name == port_name && rp.dir == PortDir::Input)
        });
        let unit_type = port_meta.map_or(UnitType::default(), |rp| rp.unit_type);
        let preview_irrelevant = port_meta.is_some_and(|rp| rp.preview_value.is_some());
        let thumbnail_relevant = port_meta.is_some_and(|rp| rp.persist_in_thumbnail);

        let port_value = unit_type.from_display(display_value);

        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .set_port_default(nid, port_name, port_value)
            .map_err(|e| format!("{e}"))?;
        let kind = if preview_irrelevant {
            ChangeKind::PreviewIrrelevantScrub
        } else if thumbnail_relevant {
            ChangeKind::ThumbnailRelevantScrub
        } else {
            ChangeKind::ScrubOnly
        };
        self.compile_active(kind)?;
        Ok(self.active_graph_json())
    }

    /// Toggle whether a port is exposed in the brush properties panel.
    /// Metadata-only — no compile needed (exposed flag doesn't affect
    /// rendered output) — but bump the topology version so the frontend
    /// treats this as a structural change and clears the active preset
    /// name. Bumping the graph version too keeps the editor preview
    /// consistent with other graph mutations.
    pub fn brush_graph_set_port_exposed(
        &mut self,
        node_id: u64,
        port_name: &str,
        exposed: bool,
    ) -> Result<String, String> {
        self.tool_session
            .write()
            .get_mut::<BrushState>()
            .expect(NO_BRUSH_STATE)
            .graph
            .set_port_exposed(NodeId(node_id), port_name, exposed)
            .map_err(|e| format!("{e}"))?;
        self.bump_brush_topology_version();
        Ok(self.active_graph_json())
    }
}

// ── Exposed port types ──────────────────────────────────────────────

/// Type-specific value data for an exposed port.
///
/// Tagged enum so the frontend can switch on `kind` to render the
/// appropriate widget (scrub slider, toggle, color picker, etc.).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExposedValue {
    /// Float scrub slider with unit conversion.
    Scalar {
        /// Current value in display-space.
        value: f32,
        /// Display-space minimum.
        min: f32,
        /// Display-space maximum.
        max: f32,
        /// Display-space default — what double-click reset returns to.
        /// Sourced from the node-type registration, not the loaded brush.
        default: f32,
        /// Unit type for formatting and conversion.
        #[serde(rename = "unitType")]
        unit_type: UnitType,
    },
    // Future variants:
    // Int { value: i32, min: i32, max: i32 },
    // Bool { value: bool },
    // Color { value: [f32; 4] },
}

/// Info about an exposed port — sent to the frontend for the BrushBar.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExposedPortInfo {
    pub node_id: u64,
    pub port_name: String,
    pub label: String,
    pub icon: String,
    pub description: String,
    pub node_display_name: String,
    pub data: ExposedValue,
}
