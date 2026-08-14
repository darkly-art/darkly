use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::catalog::{Catalog, CatalogEntry};
use crate::gpu::params::ParamDef;

/// What each tool module returns from its `register()` function.
/// Contains metadata for the tool system. Follows the same auto-discovery
/// convention as `FilterRegistration` and `VeilRegistration`.
pub struct ToolRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// Iconify name for the toolbar button. A tool whose glyph depends on
    /// session state (the brush's eraser mode) overrides this in its frontend
    /// descriptor; this is the registry's own, state-free answer.
    pub icon: &'static str,
    /// One-sentence summary of what the tool does on the canvas — the toolbar
    /// tooltip and the reference manual's row for it.
    pub description: &'static str,
    /// Id of the action that selects this tool. Bindings in
    /// `presets/*.yaml` name this string, and it is deliberately not derived
    /// from `type_id` — `colorpicker` binds `colorPickerTool`.
    pub hotkey_action: &'static str,
    pub params: &'static [ParamDef],
}

/// Id of the catalog this registry projects into.
pub const CATALOG_ID: &str = "tools";

impl ToolRegistration {
    pub fn catalog_entry(&self) -> CatalogEntry {
        CatalogEntry::new(self.type_id, self.display_name)
            .with_icon(self.icon)
            .with_description(self.description)
            .with_hotkey_action(self.hotkey_action)
            .with_params(self.params)
    }
}

/// The tool catalog — every registered tool, sorted by `type_id`.
pub fn catalog() -> Catalog {
    Catalog::new(
        CATALOG_ID,
        "Tools",
        registry()
            .types()
            .into_iter()
            .map(ToolRegistration::catalog_entry)
            .collect(),
    )
    .with_description("What a pointer does on the canvas — painting, filling, picking, selecting.")
}

/// Auto-discovered tool registry. Owns the human-friendly display name surface
/// the UI consumes, plus the parameter-definition lookup used by the engine.
pub struct ToolRegistry {
    entries: HashMap<&'static str, ToolEntry>,
}

struct ToolEntry {
    /// The full registration this entry was built from. All metadata accessors
    /// read straight off this, so a new `ToolRegistration` field is exposed
    /// without widening any tuple or touching the registry.
    reg: ToolRegistration,
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
            entries.insert(reg.type_id, ToolEntry { reg });
        }
        ToolRegistry { entries }
    }

    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.reg.display_name)
            .unwrap_or("")
    }

    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries
            .get(type_id)
            .map(|e| e.reg.params)
            .unwrap_or(&[])
    }

    /// Return every registered tool's full [`ToolRegistration`], sorted by
    /// `type_id` for deterministic output. Callers read whatever fields they
    /// need off the registration — a new field is free here.
    pub fn types(&self) -> Vec<&ToolRegistration> {
        let mut v: Vec<&ToolRegistration> = self.entries.values().map(|e| &e.reg).collect();
        v.sort_by_key(|reg| reg.type_id);
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

// ---------------------------------------------------------------------------
// ToolSession — generic shared-state container for tools
// ---------------------------------------------------------------------------

/// Process-wide bag of tool state shared across every `DarklyEngine`
/// spawned from one `DarklySession`. Tools that have state which must
/// survive engine swaps — multi-tab brush graph being the motivating
/// example — register a state type here and read/write it through
/// `get::<T>()` / `get_mut::<T>()`.
///
/// The container has zero knowledge of which tools exist or what they
/// store. Each tool's state type lives in its own module (e.g.
/// [`crate::brush::state::BrushState`]); this container just hands out
/// typed references keyed by `TypeId`.
///
/// Tools whose state is *per-document* (selection mask, transform
/// floating layer, future clone-tool source) belong on the document or
/// engine, not here. The `ToolSession` is exclusively for state that
/// every engine should see the same.
pub struct ToolSession {
    states: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl ToolSession {
    pub fn new() -> Self {
        ToolSession {
            states: HashMap::new(),
        }
    }

    /// Install a tool's state type into the session, replacing any prior
    /// entry of the same type. Typically called once when the session is
    /// constructed.
    pub fn insert<T: 'static + Send + Sync>(&mut self, value: T) {
        self.states.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Borrow a tool's state by type. Returns `None` if no entry for
    /// `T` was installed.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.states.get(&TypeId::of::<T>())?.downcast_ref::<T>()
    }

    /// Mutably borrow a tool's state by type.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.states.get_mut(&TypeId::of::<T>())?.downcast_mut::<T>()
    }

    /// Get a mutable handle to a tool's state, inserting `T::default()`
    /// when absent. Useful for lazy registration on first access.
    pub fn get_or_default<T: 'static + Send + Sync + Default>(&mut self) -> &mut T {
        self.states
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut::<T>()
            .expect("TypeId key guarantees the stored type matches `T`")
    }
}

impl Default for ToolSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Owning handle to a shared tool session. Cheap to clone (`Arc`); every
/// clone references the same underlying `ToolSession`. The lock is
/// `RwLock` so engines can take a read guard during stroke compilation
/// while UI mutations take a write guard.
///
/// `wgpu::Device`-equivalent caveat: on `wasm32` this `Arc<RwLock<_>>`
/// pattern triggers `arc_with_non_send_sync` only if a stored value
/// isn't `Send + Sync`. Every state type registered here must be
/// `Send + Sync` (enforced by `ToolSession::insert`'s bound), so the
/// lint stays clean.
#[derive(Clone)]
pub struct SharedToolSession(Arc<RwLock<ToolSession>>);

impl SharedToolSession {
    /// Allocate a fresh empty shared session. Callers immediately
    /// `.write().insert(...)` each tool's initial state.
    pub fn new() -> Self {
        SharedToolSession(Arc::new(RwLock::new(ToolSession::new())))
    }

    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, ToolSession> {
        self.0.read().expect("tool session lock poisoned")
    }

    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, ToolSession> {
        self.0.write().expect("tool session lock poisoned")
    }
}

impl Default for SharedToolSession {
    fn default() -> Self {
        Self::new()
    }
}
