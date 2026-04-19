//! Brush graph management methods on DarklyEngine.
//!
//! Provides the API surface for the WASM bridge to query node types,
//! get/set the active brush graph, and compile graphs.

use super::DarklyEngine;
use crate::brush::wire::BrushWireType;
use crate::brush::{BrushNodeRegistration, BrushNodeRegistry};
use crate::gpu::params::ParamValue;
use crate::nodegraph::{NodeId, PortDir, PortRef, UnitType};
use crate::nodegraph::Graph;

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
        // Run the post-mutation pipeline so the brush preview mask (and any
        // other graph-dependent state) refreshes from the new graph.
        self.compile_active()?;
        Ok(())
    }

    /// Reset the active brush graph to the built-in default.
    pub fn reset_brush_graph(&mut self) {
        self.active_brush_graph = crate::brush::default_graph();
        let _ = self.compile_active();
    }

    // --- Fine-grained graph commands ---

    /// Re-render the brush preview into the overlay's preview mask using
    /// fully-synthetic pen inputs. Fired on graph/param changes where no
    /// real pen data is available.
    pub fn regenerate_brush_preview(&mut self) {
        let dummy = crate::brush::paint_info::PaintInformation::preview_dummy();
        self.regenerate_brush_preview_with_pen(&dummy);
    }

    /// Re-render the brush preview using the supplied pen data.
    ///
    /// Called by the brush tool on hover so the preview reflects live tilt
    /// / rotation / pressure. `pen` carries whatever the PointerEvent
    /// reported; fields the hardware doesn't populate should be zeroed
    /// (tablet quirk: most pens report tilt even while hovering above the
    /// canvas, so we plumb them through).
    ///
    /// Runs the active brush graph normally — CPU eval, GPU eval — but
    /// with `render_mode: Preview`. `color_output` bails in that mode;
    /// `preview_output` (if the graph has one) blits its upstream dab
    /// into the overlay's preview mask. Positioning info is read from
    /// `preview_output`'s resolved input slots after eval.
    ///
    /// No-op with cleared state when the graph has no `preview_output`.
    pub fn regenerate_brush_preview_with_pen(
        &mut self,
        pen: &crate::brush::paint_info::PaintInformation,
    ) {
        use crate::brush::gpu_context::{BrushGpuContext, RenderMode};

        let mut runner = match crate::brush::compile_graph(&self.active_brush_graph) {
            Ok(r) => r,
            Err(_) => {
                self.compositor.clear_overlay_preview_mask();
                self.brush_preview_info = None;
                return;
            }
        };

        if !runner.has_preview_terminal() {
            self.compositor.clear_overlay_preview_mask();
            self.brush_preview_info = None;
            return;
        }

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
        let encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("brush-preview-regen") },
        );

        let mut gpu_ctx = BrushGpuContext {
            encoder,
            device: &self.gpu.device,
            queue: &self.gpu.queue,
            dab_pool: &mut self.dab_pool,
            pipelines: &self.brush_pipelines,
            // color_output bails in Preview mode — canvas_view is unused but
            // the struct requires it. Reuse the preview view as a placeholder.
            canvas_view: &target_view,
            canvas_texture: preview_tex,
            canvas_width: target_size.0,
            canvas_height: target_size.1,
            selection_bind_group: sel_bg,
            resource_handles: &self.resource_handles,
            blend_mode: 0,
            canvas_copy_origin: None,
            render_mode: RenderMode::Preview,
            preview_target_view: Some(&target_view),
            preview_target_size: target_size,
        };

        self.brush_pipelines.reset_uniform_rings();
        runner.clear_slots();
        runner.seed_sensors(pen, [1.0, 1.0, 1.0, 1.0], 0, 0);
        runner.execute_cpu();
        runner.execute_gpu(&mut gpu_ctx);
        let info = runner.read_preview_info().unwrap_or_default();

        gpu_ctx.dab_pool.release_all();
        let command_buf = gpu_ctx.encoder.finish();
        self.gpu.queue.submit([command_buf]);

        self.compositor.use_overlay_preview_mask();
        self.brush_preview_info = Some(info);
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
        crate::brush::compile_graph(&self.active_brush_graph)
            .map_err(|e| format!("{e}"))?;

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

        // Refresh the brush preview overlay now that the graph is compiled —
        // size, rotation, and tip changes all land here.
        self.regenerate_brush_preview();

        Ok(())
    }

    /// Serialize the active graph as JSON.
    fn active_graph_json(&self) -> String {
        serde_json::to_string(&self.active_brush_graph)
            .unwrap_or_else(|_| "null".into())
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
        let _ = self.active_brush_graph.set_node_position(NodeId(node_id), [x, y]);
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
        self.resource_handles.insert(resource_name.to_string(), handle);
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
                let connected = self.active_brush_graph.connections.iter().any(|c| {
                    c.to.node == node.id && c.to.port == port.name
                });
                if connected {
                    continue;
                }

                // Display metadata comes from the registration (canonical),
                // per-instance state (default, exposed) from the instance.
                let reg_port = reg.and_then(|r| {
                    r.ports.iter().find(|rp| rp.name == port.name && rp.dir == port.dir)
                });
                let unit_type = reg_port.map_or(port.unit_type, |rp| rp.unit_type);
                let label = reg_port
                    .map(|rp| &rp.label)
                    .filter(|l| !l.is_empty())
                    .cloned()
                    .unwrap_or_else(|| port.name.clone());
                let icon = reg_port.map_or_else(
                    || port.icon.clone(),
                    |rp| rp.icon.clone(),
                );
                let description = reg_port.map_or_else(
                    || port.description.clone(),
                    |rp| rp.description.clone(),
                );

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
            1 => UnitType::Raw,     // pixels (display as-is)
            2 => UnitType::Degrees,
            3 => UnitType::Raw,
            _ => UnitType::Percent, // 0 = percent
        };

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
            .and_then(|r| r.ports.iter().find(|rp| rp.name == port_name && rp.dir == PortDir::Input))
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
