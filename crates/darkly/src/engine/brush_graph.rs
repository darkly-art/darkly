//! Brush graph management methods on DarklyEngine.
//!
//! Provides the API surface for the WASM bridge to query node types,
//! get/set the active brush graph, and compile graphs.

use super::DarklyEngine;
use crate::brush::wire::BrushWireType;
use crate::brush::{BrushNodeRegistration, BrushNodeRegistry};
use crate::gpu::params::ParamValue;
use crate::nodegraph::{NodeId, PortRef};
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
        Ok(())
    }

    /// Reset the active brush graph to the built-in default.
    pub fn reset_brush_graph(&mut self) {
        self.active_brush_graph = crate::brush::default_graph();
    }

    // --- Fine-grained graph commands ---

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

    /// Update a node's position (UI-only, no compile).
    pub fn brush_graph_move_node(&mut self, node_id: u64, x: f32, y: f32) {
        let _ = self.active_brush_graph.set_node_position(NodeId(node_id), [x, y]);
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

    /// Set the global brush scale multiplier.
    ///
    /// This controls the canvas footprint of the brush independently from
    /// the node graph's internal rendering resolution.  The value is
    /// multiplicative: 1.0 = dab pixels map 1:1, 2.0 = brush is twice
    /// as large on canvas, etc.
    pub fn set_brush_scale(&mut self, scale: f32) {
        self.brush_global_scale = scale.max(0.01);
    }

    /// Get the current global brush scale.
    pub fn brush_scale(&self) -> f32 {
        self.brush_global_scale
    }

    /// Return info about all `user_input` nodes in the active brush graph.
    ///
    /// The result is ordered by node position (top-to-bottom, left-to-right)
    /// for a stable, creator-controlled layout in the properties panel.
    pub fn brush_user_inputs(&self) -> Vec<UserInputInfo> {
        let mut inputs: Vec<UserInputInfo> = self
            .active_brush_graph
            .nodes
            .iter()
            .filter(|(_, node)| node.type_id == "user_input")
            .map(|(_, node)| {
                let label = node
                    .params
                    .first()
                    .and_then(|p| match p {
                        ParamValue::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let value = node
                    .params
                    .get(1)
                    .and_then(|p| match p {
                        ParamValue::Float(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0.5);
                UserInputInfo {
                    node_id: node.id.0,
                    label,
                    value,
                    position: node.position,
                }
            })
            .collect();

        // Sort by position: top-to-bottom (y), then left-to-right (x).
        inputs.sort_by(|a, b| {
            a.position[1]
                .partial_cmp(&b.position[1])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.position[0]
                        .partial_cmp(&b.position[0])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        inputs
    }
}

/// Info about a `user_input` node — exposed to the frontend for the
/// brush properties panel.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputInfo {
    pub node_id: u64,
    pub label: String,
    pub value: f32,
    pub position: [f32; 2],
}
