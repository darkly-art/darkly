use std::collections::HashMap;
use std::sync::OnceLock;

use crate::gpu::params::ParamDef;

/// What each tool module returns from its `register()` function.
/// Contains metadata for the tool system. Follows the same auto-discovery
/// convention as `FilterRegistration` and `VeilRegistration`.
pub struct ToolRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
}

/// Auto-discovered tool registry. Owns the human-friendly display name surface
/// the UI consumes, plus the parameter-definition lookup used by the engine.
pub struct ToolRegistry {
    entries: HashMap<&'static str, ToolEntry>,
}

struct ToolEntry {
    display_name: &'static str,
    params: &'static [ParamDef],
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in crate::tools::registrations() {
            entries.insert(
                reg.type_id,
                ToolEntry {
                    display_name: reg.display_name,
                    params: reg.params,
                },
            );
        }
        ToolRegistry { entries }
    }

    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.display_name)
            .unwrap_or("")
    }

    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries.get(type_id).map(|e| e.params).unwrap_or(&[])
    }

    /// Return every registered tool as `(type_id, display_name, params)`,
    /// sorted by `type_id` for deterministic output.
    pub fn types(&self) -> Vec<(&'static str, &'static str, &'static [ParamDef])> {
        let mut v: Vec<_> = self
            .entries
            .iter()
            .map(|(&id, e)| (id, e.display_name, e.params))
            .collect();
        v.sort_by_key(|(id, _, _)| *id);
        v
    }
}

/// Lazily-initialized process-wide tool registry. All entries are `&'static`,
/// so a singleton avoids threading a registry handle through every code path
/// that needs to render or look up a tool's display name.
pub fn registry() -> &'static ToolRegistry {
    static REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ToolRegistry::new)
}
