//! Void (procedural-content layer) queries.

use darkly_macros::handlers;

use super::DarklyEngine;
use crate::gpu::params::ParamDef;

#[handlers]
impl DarklyEngine {
    // --- Queries ---

    /// Get the parameter definitions for a void type.
    pub fn void_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.void_registry().param_defs(type_id)
    }

    /// Coerce a raw JSON params object into typed [`ParamValue`]s against a void
    /// type's `ParamDef` schema. The single param-coercion seam every
    /// void-param handler shares (add / update): it pairs the raw `params` with
    /// the sibling `void_type` that names the schema, which is exactly the
    /// pairing generic request routing can't do for itself.
    ///
    /// [`ParamValue`]: crate::gpu::params::ParamValue
    pub fn coerce_void_params(
        &self,
        type_id: &str,
        params: &serde_json::Value,
    ) -> Vec<crate::gpu::params::ParamValue> {
        crate::gpu::params::param_values_from_json(params, self.void_param_defs(type_id))
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
}
