//! Veil (post-processing filter) management and query methods.

use super::types::{
    node_to_layer_info, BlendModeTypeInfo, LayerInfo, LayerKindTypeInfo, ModifierTypeInfo,
    ParamInfo, ToolTypeInfo, VeilInfo, VeilTypeInfo,
};
use super::DarklyEngine;
use crate::gpu::params::{ParamDef, ParamValue};

impl DarklyEngine {
    // --- Veils ---

    pub fn add_veil(&mut self, veil_type: &str, params: &[ParamValue]) {
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let veil = chain
            .registry_mut()
            .create_veil(veil_type, params, &self.gpu.device, format);
        chain.add_veil(&self.gpu.device, &self.gpu.queue, veil);
    }

    pub fn remove_veil(&mut self, index: usize) {
        self.compositor.veil_chain_mut().remove_veil(index);
    }

    pub fn clear_veils(&mut self) {
        self.compositor.veil_chain_mut().clear_veils();
    }

    pub fn set_veil_visible(&mut self, index: usize, visible: bool) {
        self.compositor
            .veil_chain_mut()
            .set_veil_visible(index, visible);
    }

    pub fn move_veil(&mut self, from: usize, to: usize) {
        self.compositor.veil_chain_mut().move_veil(from, to);
    }

    pub fn update_veil(&mut self, index: usize, params: &[ParamValue]) {
        let type_id: &'static str = match self.compositor.veil_chain().type_id(index) {
            Some(t) => t,
            None => return,
        };
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let new_veil = chain
            .registry_mut()
            .create_veil(type_id, params, &self.gpu.device, format);
        chain.update_veil(&self.gpu.device, &self.gpu.queue, index, new_veil);
    }

    // --- Queries ---

    pub fn layer_tree(&self) -> Vec<LayerInfo> {
        self.doc
            .children_of(self.doc.root_id())
            .iter()
            .rev()
            .filter_map(|id| node_to_layer_info(&self.doc, self.compositor.void_registry(), *id))
            .collect()
    }

    pub fn veil_list(&self) -> Vec<VeilInfo> {
        let chain = self.compositor.veil_chain();
        let count = chain.count();
        let mut list = Vec::with_capacity(count);
        for i in (0..count).rev() {
            if let Some((type_id, visible)) = chain.info(i) {
                let param_defs = chain.registry().param_defs(type_id);
                let values = chain.param_values(i).unwrap_or_default();
                let params = param_defs
                    .iter()
                    .enumerate()
                    .map(|(j, def)| ParamInfo::from_def(def, values.get(j)))
                    .collect();
                list.push(VeilInfo {
                    type_id: type_id.to_string(),
                    visible,
                    index: i,
                    params,
                });
            }
        }
        list
    }

    /// Return all registered veil types with their parameter definitions.
    pub fn veil_types(&self) -> Vec<VeilTypeInfo> {
        self.compositor
            .veil_chain()
            .registry()
            .types()
            .into_iter()
            .map(|(type_id, display_name, defs)| VeilTypeInfo {
                type_id,
                display_name,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Get the parameter definitions for a veil type.
    pub fn veil_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.veil_chain().registry().param_defs(type_id)
    }

    /// Return all registered void types with their parameter definitions.
    /// Same shape as `veil_types()` — the UI consumes both through the
    /// shared `VeilTypeInfo` struct (renamed `VoidTypeInfo` would just be a
    /// type alias; reusing the existing one keeps the JSON identical and
    /// the frontend's render code generic).
    pub fn void_types(&self) -> Vec<VeilTypeInfo> {
        self.compositor
            .void_registry()
            .types()
            .into_iter()
            .map(|(type_id, display_name, defs)| VeilTypeInfo {
                type_id,
                display_name,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Get the parameter definitions for a void type.
    pub fn void_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.void_registry().param_defs(type_id)
    }

    /// Resolve a layer id to its void type, if the layer is a void.
    /// Helper for the WASM bridge so callers don't need to import the layer
    /// enum to query the active void's schema.
    pub fn void_layer_type(&self, layer_id: crate::layer::LayerId) -> Option<String> {
        match self.doc.find_node(layer_id)? {
            crate::layer::LayerNode::Layer(crate::layer::Layer::Void(v)) => {
                Some(v.void_type.clone())
            }
            _ => None,
        }
    }

    /// Return all registered tool types with display name and parameter definitions.
    /// Backs the WASM bridge so the UI can render tool names without hardcoding them.
    pub fn tool_types(&self) -> Vec<ToolTypeInfo> {
        crate::tool::registry()
            .types()
            .into_iter()
            .map(|(type_id, display_name, defs)| ToolTypeInfo {
                type_id,
                display_name,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Return all registered blend modes in GPU-value order, with display name
    /// and category. Backs the WASM bridge so the UI populates the blend-mode
    /// dropdown from the registry instead of a hardcoded table.
    pub fn blend_mode_types(&self) -> Vec<BlendModeTypeInfo> {
        crate::gpu::blend_mode::registry()
            .all()
            .into_iter()
            .map(|reg| BlendModeTypeInfo {
                type_id: reg.type_id,
                display_name: reg.display_name,
                category: reg.category,
            })
            .collect()
    }

    /// Return all registered modifier kinds. UI uses this to resolve
    /// `ModifierInfo.kind` to a display label and to populate the
    /// "Add modifier" menu.
    pub fn modifier_types(&self) -> Vec<ModifierTypeInfo> {
        crate::document::modifier::registry()
            .all()
            .into_iter()
            .map(|reg| ModifierTypeInfo {
                type_id: reg.type_id,
                display_name: reg.display_name,
            })
            .collect()
    }

    /// Return all registered layer kinds. UI uses this to resolve a layer's
    /// `type` discriminator to a display label (e.g. "Raster Layer", "Group").
    pub fn layer_kind_types(&self) -> Vec<LayerKindTypeInfo> {
        crate::document::layer_kind::registry()
            .all()
            .into_iter()
            .map(|reg| LayerKindTypeInfo {
                type_id: reg.type_id,
                display_name: reg.display_name,
            })
            .collect()
    }
}
