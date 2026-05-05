//! Veil (post-processing filter) management and query methods.

use super::types::{node_to_layer_info, LayerInfo, ParamInfo, VeilInfo, VeilTypeInfo};
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
            .filter_map(|id| node_to_layer_info(&self.doc, *id))
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
            .map(|(type_id, defs)| VeilTypeInfo {
                type_id,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Get the parameter definitions for a veil type.
    pub fn veil_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.veil_chain().registry().param_defs(type_id)
    }
}
