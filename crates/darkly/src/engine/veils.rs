//! Veil (post-processing filter) management and query methods.

use darkly_macros::handlers;

use super::types::{node_to_layer_info, LayerInfo, ParamInfo, VeilInfo};
use super::DarklyEngine;
use crate::catalog::Catalog;
use crate::engine::protocol::{params_from_json, RawParams};
use crate::gpu::params::{ParamDef, ParamValue};

#[handlers]
impl DarklyEngine {
    // --- Veils ---

    /// Wire entry for `add_veil` — coerces JSON `params` against the veil
    /// type's schema, then [`Self::add_veil_layer`].
    #[handler]
    pub fn add_veil(&mut self, veil_type: String, params: RawParams) {
        let pv = params_from_json(&params.0, self.veil_param_defs(&veil_type));
        self.add_veil_layer(&veil_type, &pv);
    }

    /// Wire entry for `update_veil` — resolves the slot's veil type, coerces
    /// `params` against its schema, then [`Self::update_veil_layer`]. A stale
    /// index is a silent no-op.
    #[handler]
    pub fn update_veil(&mut self, index: usize, params: RawParams) {
        let Some(type_id) = self.compositor.veil_chain().type_id(index) else {
            return;
        };
        let pv = params_from_json(&params.0, self.veil_param_defs(type_id));
        self.update_veil_layer(index, &pv);
    }

    pub fn add_veil_layer(&mut self, veil_type: &str, params: &[ParamValue]) {
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let veil = chain
            .registry_mut()
            .create_veil(veil_type, params, &self.gpu.device, format);
        chain.add_veil(&self.gpu.device, &self.gpu.queue, veil);
    }

    #[handler]
    pub fn remove_veil(&mut self, index: usize) {
        self.compositor.veil_chain_mut().remove_veil(index);
    }

    #[handler]
    pub fn clear_veils(&mut self) {
        self.compositor.veil_chain_mut().clear_veils();
    }

    #[handler]
    pub fn set_veil_visible(&mut self, index: usize, visible: bool) {
        self.compositor
            .veil_chain_mut()
            .set_veil_visible(index, visible);
    }

    #[handler]
    pub fn move_veil(&mut self, from: usize, to: usize) {
        self.compositor.veil_chain_mut().move_veil(from, to);
    }

    pub fn update_veil_layer(&mut self, index: usize, params: &[ParamValue]) {
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

    #[handler]
    pub fn layer_tree(&self) -> Vec<LayerInfo> {
        self.doc
            .children_of(self.doc.root_id())
            .iter()
            .rev()
            .filter_map(|id| {
                node_to_layer_info(
                    &self.doc,
                    self.compositor.void_registry(),
                    self.compositor.filter_pipeline_registry(),
                    *id,
                )
            })
            .collect()
    }

    #[handler]
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

    /// Every registry, projected into the one browsable shape the UI pickers,
    /// the settings surface and the metadata export all consume. Delegates to
    /// the GPU-free free function so an exporter can build the same data
    /// without an engine; the handler exists so `ts_rs` emits `Catalog` into
    /// the frontend's typed client.
    #[handler]
    pub fn catalogs(&self) -> Vec<Catalog> {
        crate::catalog::catalogs()
    }

    /// Get the parameter definitions for a veil type.
    pub fn veil_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.veil_chain().registry().param_defs(type_id)
    }
}
