use crate::document::layer_kind::LayerKindRegistration;

/// Stable wire-format identifier. Owned by this module so dispatch sites
/// (notably `LayerNode::kind`) reference the same constant the registration
/// uses — no parallel string literal anywhere.
pub const TYPE_ID: &str = "raster";

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Raster Layer",
    }
}
