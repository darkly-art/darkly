//! Brush graph management methods on DarklyEngine.
//!
//! Provides the API surface for the WASM bridge to query node types,
//! get/set the active brush graph, and compile graphs.

use super::{DarklyEngine, ReadbackContext};
use crate::brush::wire::BrushWireType;
use crate::brush::{BrushNodeRegistration, BrushNodeRegistry};
use crate::gpu::params::ParamValue;
use crate::nodegraph::Graph;
use crate::nodegraph::{NodeId, PortDir, PortRef, UnitType};

impl DarklyEngine {
    /// Return metadata for all registered brush node types.
    pub fn brush_node_types(&self) -> Vec<BrushNodeRegistration> {
        let registry = BrushNodeRegistry::new();
        registry.types().cloned().collect()
    }

    /// Return a clone of the default brush graph.
    pub fn default_brush_graph(&self) -> Graph<BrushWireType> {
        crate::brush::default_graph()
    }

    /// Return a reference to the currently active brush graph.
    pub fn active_brush_graph_ref(&self) -> &Graph<BrushWireType> {
        &self.active_brush_graph
    }

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
        self.active_brush_graph = graph;
        self.snapshot_brush_defaults();
        // Run the post-mutation pipeline so the brush preview mask (and any
        // other graph-dependent state) refreshes from the new graph.
        self.compile_active()?;
        Ok(())
    }

    /// Reset the active brush graph to the built-in default.
    pub fn reset_brush_graph(&mut self) {
        self.active_brush_graph = crate::brush::default_graph();
        self.snapshot_brush_defaults();
        let _ = self.compile_active();
    }

    /// Capture every input port's current default into `brush_defaults`.
    /// Called whenever the active graph is replaced as a whole — brush
    /// load, reset, save — so that "reset to default" returns to the
    /// loaded/saved baseline rather than the node-type registration value.
    /// Not called on individual port edits; that's the whole point.
    ///
    /// Also captures the legacy `user_input` node's `value` param under
    /// a synthetic ("value") key — that node surfaces in the toolbar via
    /// the legacy compat path and needs the same reset semantics.
    pub(crate) fn snapshot_brush_defaults(&mut self) {
        self.brush_defaults.clear();
        for node in self.active_brush_graph.nodes.values() {
            for port in &node.ports {
                if port.dir == PortDir::Input {
                    self.brush_defaults
                        .insert((node.id, port.name.clone()), port.default);
                }
            }
            if node.type_id == "user_input" {
                if let Some(ParamValue::Float(v)) = node.params.get(1) {
                    self.brush_defaults
                        .insert((node.id, "value".to_string()), *v);
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
        use crate::brush::gpu_context::BrushGpuContext;

        let mut runner = match crate::brush::compile_graph(&self.active_brush_graph) {
            Ok(r) => r,
            Err(_) => {
                self.compositor.clear_overlay_preview_mask();
                self.brush_preview_info = None;
                return;
            }
        };

        // Always dispatch `render_preview` — individual terminals decide
        // whether they produce output this frame. A paint graph with no
        // `brush_preview` wire has color_output's hook return early and
        // `brush_preview_info` stays None; a self-previewing terminal
        // (liquify etc.) fires its hook and publishes placement info. The
        // post-run `info.is_some()` check below routes both outcomes.

        // Fixed-size preview mask; overlay's linear sampler handles display
        // scaling via the primitive's canvas-space half-extent.
        let target_size = (128_u32, 128_u32);
        let target_view = self
            .compositor
            .ensure_overlay_preview_mask(&self.gpu.device, target_size.0, target_size.1)
            .clone();
        let preview_tex = self
            .compositor
            .overlay_preview_mask_texture()
            .expect("ensure_overlay_preview_mask just allocated it");

        let sel_bg = if self.gpu_selection.active {
            self.gpu_selection.brush_bind_group()
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
            dab_pool: &mut self.dab_pool,
            pipelines: &self.brush_pipelines,
            // The preview pipeline doesn't touch the stroke scratch — the
            // terminal's `render_preview` writes to `preview_mask_view`
            // instead. Alias the scratch fields to the preview target so
            // the struct is well-formed (no Option needed).
            stroke_scratch_view: &target_view,
            stroke_scratch_texture: preview_tex,
            canvas_width: target_size.0,
            canvas_height: target_size.1,
            selection_bind_group: sel_bg,
            resource_handles: &self.resource_handles,
            blend_mode: 0,
            canvas_copy_origin: None,
            preview_mask_view: Some(&target_view),
            preview_mask_size: target_size,
            brush_preview_info: None,
            // No layer / pre-stroke state in preview — commit isn't called.
            layer_view: None,
            layer_texture: None,
            pre_stroke_texture: None,
            pre_stroke_bind_group: None,
            scratch_bind_group: None,
            dab_write_bbox: None,
        };

        self.brush_pipelines.reset_uniform_rings();
        runner.clear_slots();
        runner.seed_sensors(&pen, [1.0, 1.0, 1.0, 1.0], 0, 0);
        runner.execute_cpu();
        runner.render_preview_pipeline(&mut gpu_ctx);

        let info = gpu_ctx.brush_preview_info;
        gpu_ctx.dab_pool.release_all();
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

    /// Compile the active graph in-place, then release any static GPU
    /// textures that are no longer referenced by an Image node.
    ///
    /// Returns Ok on success or an error string.
    fn compile_active(&mut self) -> Result<(), String> {
        crate::brush::compile_graph(&self.active_brush_graph).map_err(|e| format!("{e}"))?;

        // Collect resource names still referenced by Image nodes.
        let live: std::collections::HashSet<String> = self
            .active_brush_graph
            .nodes
            .values()
            .filter(|n| n.type_id == "image")
            .filter_map(|n| match n.params.first() {
                Some(ParamValue::String(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
            .collect();

        // Release static textures whose resource name is no longer live.
        let stale: Vec<String> = self
            .resource_handles
            .keys()
            .filter(|name| !live.contains(name.as_str()))
            .cloned()
            .collect();
        for name in stale {
            if let Some(handle) = self.resource_handles.remove(&name) {
                self.dab_pool.release_static(handle);
            }
        }

        // Every compile bumps the version so the editor preview knows to
        // re-render and stale in-flight readbacks can be discarded on arrival.
        self.brush_graph_version = self.brush_graph_version.wrapping_add(1);

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
    /// cached bytes synchronously. The pixels update on a later frame once
    /// the async readback completes — same shape as `layer_thumbnail`.
    ///
    /// Uses the theme colors stored via `set_preview_theme`, not the user's
    /// active paint color — keeps the editor preview visually consistent
    /// with the brush picker's brush thumbnails.
    pub fn brush_editor_preview(&mut self, width: u32, height: u32) -> Vec<u8> {
        // Guard against painting while a real stroke is in flight — the
        // preview shares `dab_pool` and `brush_pipelines` with the engine,
        // and running mid-stroke would step on acquired handles and
        // uniform rings.
        let in_stroke = self.brush_stroke_engine.is_some();

        let zero_buffer = || vec![0u8; (width * height * 4) as usize];
        let cached = self
            .brush_editor_preview_cache
            .clone()
            .filter(|_| self.brush_editor_preview_cache_size == Some((width, height)));

        // Skip work when nothing has changed and the cache is good. Also
        // skip if a real stroke is in progress — return the most recent
        // cached bytes so the UI stays responsive without clobbering the
        // stroke's GPU state.
        let nothing_to_do = in_stroke
            || (self.last_rendered_preview_version == self.brush_graph_version
                && self.brush_editor_preview_cache_size == Some((width, height)));
        if nothing_to_do {
            return cached.unwrap_or_else(zero_buffer);
        }

        // Don't queue a second readback on top of an in-flight one — it
        // would race with whichever lands first and the stale result
        // could overwrite the fresh one.
        let already_pending = self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::BrushEditorPreview { .. }));
        if already_pending {
            return cached.unwrap_or_else(zero_buffer);
        }

        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;

        // Clone the active graph so we can pass it through the shared
        // helper without holding a borrow on `self` across the call.
        let graph = self.active_brush_graph.clone();
        let path = crate::brush::preview_renderer::synthesize_preview_stroke(
            width as f32,
            height as f32,
            30,
        );
        self.render_preview_and_request_readback(
            &graph,
            &path,
            width,
            height,
            fg,
            bg,
            ReadbackContext::BrushEditorPreview {
                width,
                height,
                graph_version: self.brush_graph_version,
            },
        );
        self.last_rendered_preview_version = self.brush_graph_version;

        cached.unwrap_or_else(zero_buffer)
    }

    /// Invalidate any cached editor preview — call when the theme colors
    /// change so the next `brush_editor_preview` request re-renders with
    /// the new palette instead of returning the stale cached pixels.
    /// Also drops the active-dab preview cache so the BrushBar trigger
    /// thumbnail and the picker's active-brush strip refresh on the same
    /// signal.
    pub fn invalidate_brush_editor_preview(&mut self) {
        self.brush_editor_preview_cache = None;
        self.brush_editor_preview_cache_size = None;
        self.active_dab_preview_cache = None;
        self.active_dab_preview_cache_size = None;
        // Bumping the version forces the skip-check in
        // `brush_editor_preview` to trigger a fresh render; also drops
        // any in-flight readback as stale when it lands.
        self.brush_graph_version = self.brush_graph_version.wrapping_add(1);
    }

    /// Render a single-dab preview of the active brush and return the
    /// most recent cached bytes synchronously. Pixels update on a later
    /// frame once the async readback completes — same shape as
    /// `brush_editor_preview` and `layer_thumbnail`. Used by the
    /// BrushBar trigger button and the picker's active-brush strip.
    pub fn brush_active_dab_preview(&mut self, width: u32, height: u32) -> Vec<u8> {
        // Guard against painting while a real stroke is in flight — the
        // preview shares `dab_pool` and `brush_pipelines` with the engine,
        // and running mid-stroke would step on acquired handles and
        // uniform rings.
        let in_stroke = self.brush_stroke_engine.is_some();

        let zero_buffer = || vec![0u8; (width * height * 4) as usize];
        let cached = self
            .active_dab_preview_cache
            .clone()
            .filter(|_| self.active_dab_preview_cache_size == Some((width, height)));

        // Skip work when nothing has changed and the cache is good. Also
        // skip while a real stroke is in progress — return the most recent
        // cached bytes so the UI stays responsive without clobbering the
        // stroke's GPU state.
        let nothing_to_do = in_stroke
            || (self.last_rendered_dab_version == self.brush_graph_version
                && self.active_dab_preview_cache_size == Some((width, height)));
        if nothing_to_do {
            return cached.unwrap_or_else(zero_buffer);
        }

        // Don't queue a second readback on top of an in-flight one.
        let already_pending = self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::ActiveBrushDab { .. }));
        if already_pending {
            return cached.unwrap_or_else(zero_buffer);
        }

        let fg = self.preview_theme_fg;
        let bg = self.preview_theme_bg;
        let graph = self.active_brush_graph.clone();
        let path =
            crate::brush::preview_renderer::synthesize_preview_dab(width as f32, height as f32);
        self.render_preview_and_request_readback(
            &graph,
            &path,
            width,
            height,
            fg,
            bg,
            ReadbackContext::ActiveBrushDab {
                width,
                height,
                graph_version: self.brush_graph_version,
            },
        );
        self.last_rendered_dab_version = self.brush_graph_version;

        cached.unwrap_or_else(zero_buffer)
    }

    /// Shared helper: render a preview path into the preview renderer's
    /// texture, then encode an async readback tagged with `context`. The
    /// caller decides what to do with the bytes when they arrive. The
    /// graph is taken explicitly so callers can render thumbnails for
    /// library brushes without touching the active graph; the path lets
    /// callers choose between the S-curve stroke and a single-dab preview.
    #[allow(clippy::too_many_arguments)]
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
            &mut self.dab_pool,
            &self.brush_pipelines,
            &self.resource_handles,
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
            [0, 0, width, height],
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.readbacks.submit(request, context);
    }

    /// Serialize the active graph as JSON.
    fn active_graph_json(&self) -> String {
        serde_json::to_string(&self.active_brush_graph).unwrap_or_else(|_| "null".into())
    }

    /// Add a node to the active graph and compile.
    /// Returns the updated graph JSON on success.
    pub fn brush_graph_add_node(
        &mut self,
        type_id: &str,
        x: f32,
        y: f32,
    ) -> Result<String, String> {
        let registry = BrushNodeRegistry::new();
        let reg = registry
            .get(type_id)
            .ok_or_else(|| format!("unknown node type: {type_id}"))?;

        let params = reg
            .params
            .iter()
            .map(|p| p.default_value())
            .collect::<Vec<_>>();
        let id = self
            .active_brush_graph
            .add_node(type_id, reg.ports.clone(), params);

        // Set position.
        let _ = self.active_brush_graph.set_node_position(id, [x, y]);

        self.compile_active()?;
        Ok(self.active_graph_json())
    }

    /// Remove a node from the active graph and compile.
    pub fn brush_graph_remove_node(&mut self, node_id: u64) -> Result<String, String> {
        self.active_brush_graph
            .remove_node(NodeId(node_id))
            .map_err(|e| format!("{e}"))?;
        self.compile_active()?;
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
        // Remove any existing connection to this input first.
        let to_ref = PortRef {
            node: NodeId(to_node),
            port: to_port.into(),
        };
        self.active_brush_graph
            .connections
            .retain(|c| c.to != to_ref);

        self.active_brush_graph
            .connect(
                PortRef {
                    node: NodeId(from_node),
                    port: from_port.into(),
                },
                to_ref.clone(),
            )
            .map_err(|e| format!("{e}"))?;
        self.compile_active()?;
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
        self.active_brush_graph.disconnect(
            &PortRef {
                node: NodeId(from_node),
                port: from_port.into(),
            },
            &PortRef {
                node: NodeId(to_node),
                port: to_port.into(),
            },
        );
        self.compile_active()?;
        Ok(self.active_graph_json())
    }

    /// Update a parameter on a node and compile.
    pub fn brush_graph_set_param(
        &mut self,
        node_id: u64,
        param_index: usize,
        value: ParamValue,
    ) -> Result<String, String> {
        self.active_brush_graph
            .set_param(NodeId(node_id), param_index, value)
            .map_err(|e| format!("{e}"))?;
        self.compile_active()?;
        Ok(self.active_graph_json())
    }

    /// Update a port's default value and compile.
    pub fn brush_graph_set_port_default(
        &mut self,
        node_id: u64,
        port_name: &str,
        value: f32,
    ) -> Result<String, String> {
        self.active_brush_graph
            .set_port_default(NodeId(node_id), port_name, value)
            .map_err(|e| format!("{e}"))?;
        self.compile_active()?;
        Ok(self.active_graph_json())
    }

    /// Update a node's position (UI-only, no compile).
    pub fn brush_graph_move_node(&mut self, node_id: u64, x: f32, y: f32) {
        let _ = self
            .active_brush_graph
            .set_node_position(NodeId(node_id), [x, y]);
    }

    /// Run auto-layout on the active brush graph and return updated JSON.
    /// `sizes` maps `NodeId` → `[width, height]` measured from the DOM.
    pub fn brush_graph_auto_layout(
        &mut self,
        sizes: &std::collections::HashMap<NodeId, [f32; 2]>,
    ) -> String {
        self.active_brush_graph.auto_layout_with_sizes(sizes);
        self.active_graph_json()
    }

    /// Upload an RGBA8 image and associate it with a resource name.
    ///
    /// The image is stored as a static GPU texture.  Image nodes that
    /// reference `resource_name` will output this texture's handle.
    /// If a resource with the same name already exists, it is replaced.
    pub fn brush_upload_image(
        &mut self,
        resource_name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), String> {
        // Release the old texture if replacing.
        if let Some(old) = self.resource_handles.remove(resource_name) {
            self.dab_pool.release_static(old);
        }
        let handle = self.dab_pool.upload_image(
            &self.gpu.device,
            &self.gpu.queue,
            resource_name,
            width,
            height,
            rgba,
        );
        self.resource_handles
            .insert(resource_name.to_string(), handle);
        Ok(())
    }

    /// Set the composite blend mode: 0 = source-over (paint), 1 = destination-out (erase).
    pub fn set_brush_blend_mode(&mut self, mode: u32) {
        self.brush_blend_mode = mode;
    }

    /// Return info about all exposed ports in the active brush graph.
    ///
    /// Scans all nodes for input ports with `exposed == true`, and also
    /// includes legacy `user_input` nodes for backward compatibility.
    ///
    /// The result is ordered by node position (top-to-bottom, left-to-right)
    /// for a stable, creator-controlled layout in the properties panel.
    pub fn brush_exposed_ports(&self) -> Vec<ExposedPortInfo> {
        let registry = BrushNodeRegistry::new();
        let mut result: Vec<ExposedPortInfo> = Vec::new();

        for node in self.active_brush_graph.nodes.values() {
            // Legacy user_input nodes: synthesize as exposed scalar entries.
            if node.type_id == "user_input" {
                if let Some(info) = self.legacy_user_input_to_exposed(node) {
                    result.push(info);
                    continue;
                }
            }

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
                let connected = self
                    .active_brush_graph
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
                let reset_default = self
                    .brush_defaults
                    .get(&(node.id, port.name.clone()))
                    .copied()
                    .unwrap_or_else(|| reg_port.map(|rp| rp.default).unwrap_or(port.default));
                result.push(ExposedPortInfo {
                    node_id: node.id.0,
                    port_name: port.name.clone(),
                    label,
                    icon,
                    description,
                    position: node.position,
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

        // Sort by position: top-to-bottom (y), then left-to-right (x).
        result.sort_by(|a, b| {
            a.position[1]
                .partial_cmp(&b.position[1])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.position[0]
                        .partial_cmp(&b.position[0])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        result
    }

    /// Backward compat: synthesize an `ExposedPortInfo` from a legacy
    /// `user_input` node by reading its params.
    fn legacy_user_input_to_exposed(
        &self,
        node: &crate::nodegraph::NodeInstance<BrushWireType>,
    ) -> Option<ExposedPortInfo> {
        let label = match node.params.first() {
            Some(ParamValue::String(s)) => s.clone(),
            _ => String::new(),
        };
        let value = match node.params.get(1) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.5,
        };
        let min = match node.params.get(2) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        let max = match node.params.get(3) {
            Some(ParamValue::Float(v)) => *v,
            _ => 1.0,
        };
        let units = match node.params.get(4) {
            Some(ParamValue::Int(v)) => *v as u32,
            _ => 0,
        };
        let icon = match node.params.get(5) {
            Some(ParamValue::String(s)) => s.clone(),
            _ => String::new(),
        };
        let description = match node.params.get(6) {
            Some(ParamValue::String(s)) => s.clone(),
            _ => String::new(),
        };

        // Map legacy units enum to UnitType.
        let unit_type = match units {
            1 => UnitType::Raw, // pixels (display as-is)
            2 => UnitType::Degrees,
            3 => UnitType::Raw,
            _ => UnitType::Percent, // 0 = percent
        };

        // Reset target = snapshotted brush value if available; otherwise
        // fall back to the midpoint of the user-defined range (legacy
        // user_input nodes don't have a registration default to reach
        // for, since the value is itself a node param).
        let reset_default = self
            .brush_defaults
            .get(&(node.id, "value".to_string()))
            .copied()
            .unwrap_or((min + max) * 0.5);
        Some(ExposedPortInfo {
            node_id: node.id.0,
            port_name: "value".to_string(),
            label,
            icon,
            description,
            position: node.position,
            node_display_name: "User Input".to_string(),
            data: ExposedValue::Scalar {
                value,
                min,
                max,
                default: reset_default,
                unit_type,
            },
        })
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

        // For legacy user_input nodes, delegate to param update.
        if let Some(node) = self.active_brush_graph.nodes.get(&nid) {
            if node.type_id == "user_input" && port_name == "value" {
                return self.brush_graph_set_param(
                    node_id,
                    1, // param index 1 = value
                    ParamValue::Float(display_value),
                );
            }
        }

        // Look up UnitType from the registration (canonical source).
        let node = self
            .active_brush_graph
            .nodes
            .get(&nid)
            .ok_or_else(|| format!("node {node_id} not found"))?;
        let registry = BrushNodeRegistry::new();
        let unit_type = registry
            .get(&node.type_id)
            .and_then(|r| {
                r.ports
                    .iter()
                    .find(|rp| rp.name == port_name && rp.dir == PortDir::Input)
            })
            .map_or(UnitType::default(), |rp| rp.unit_type);

        let port_value = unit_type.from_display(display_value);

        self.active_brush_graph
            .set_port_default(nid, port_name, port_value)
            .map_err(|e| format!("{e}"))?;
        self.compile_active()?;
        Ok(self.active_graph_json())
    }

    /// Toggle whether a port is exposed in the brush properties panel.
    /// Metadata-only — no compile needed, but returns updated graph JSON.
    pub fn brush_graph_set_port_exposed(
        &mut self,
        node_id: u64,
        port_name: &str,
        exposed: bool,
    ) -> Result<String, String> {
        self.active_brush_graph
            .set_port_exposed(NodeId(node_id), port_name, exposed)
            .map_err(|e| format!("{e}"))?;
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
    pub position: [f32; 2],
    pub node_display_name: String,
    pub data: ExposedValue,
}
