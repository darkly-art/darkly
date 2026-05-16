use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

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
