//! Layer-kind registry — metadata for the two structural variants of
//! [`crate::layer::LayerNode`] (`raster` and `group`).
//!
//! The structural enum stays where it is (each variant carries kind-specific
//! state). What lives *here* is the catalog of stable string `type_id`s and
//! human-friendly display names the UI uses. Adding a new layer kind is one
//! new file under [`crate::document::layer_kinds`], same as veils/tools.

use std::collections::HashMap;
use std::sync::OnceLock;

pub struct LayerKindRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
}

pub struct LayerKindRegistry {
    /// Owned storage — stable addresses while the registry lives (forever).
    entries: Vec<LayerKindRegistration>,
    by_type_id: HashMap<&'static str, usize>,
}

impl Default for LayerKindRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerKindRegistry {
    pub fn new() -> Self {
        let entries: Vec<LayerKindRegistration> = super::layer_kinds::registrations();
        let mut by_type_id = HashMap::with_capacity(entries.len());
        for (i, reg) in entries.iter().enumerate() {
            by_type_id.insert(reg.type_id, i);
        }
        LayerKindRegistry {
            entries,
            by_type_id,
        }
    }

    /// Look up by stable `type_id`. Returns `&'static` because the registry
    /// itself is — callers can hold the reference indefinitely.
    pub fn get(&'static self, type_id: &str) -> Option<&'static LayerKindRegistration> {
        self.by_type_id.get(type_id).map(|&i| &self.entries[i])
    }

    pub fn all(&'static self) -> Vec<&'static LayerKindRegistration> {
        let mut v: Vec<_> = self.entries.iter().collect();
        v.sort_by_key(|reg| reg.type_id);
        v
    }
}

/// Lazily-initialized process-wide layer-kind registry.
pub fn registry() -> &'static LayerKindRegistry {
    static REGISTRY: OnceLock<LayerKindRegistry> = OnceLock::new();
    REGISTRY.get_or_init(LayerKindRegistry::new)
}
