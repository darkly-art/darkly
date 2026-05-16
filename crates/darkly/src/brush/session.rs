//! Shared brush session — the currently-active brush graph and its
//! version counters, owned outside any single `DarklyEngine` so multiple
//! engines (e.g. the multi-tab editor) all paint with the same brush
//! without any cross-engine syncing or fan-out.
//!
//! Stored as `Arc<RwLock<BrushSession>>`. JS-side brush mutations
//! (`brush_load`, `brush_graph_add_node`, scrubs, …) target the lock
//! directly — every engine sees the change because there is only one
//! copy. Engine-side stroke processing takes a read guard for the
//! duration of compilation and discards it before painting.
//!
//! See [`crate::engine::DarklyEngine`] for the per-engine fields that
//! live separately (GPU pipelines, dab pool, in-progress
//! `brush_stroke_engine`, preview caches keyed off `version` /
//! `topology_version`).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::brush::wire::BrushWireType;
use crate::nodegraph::{Graph, NodeId};

/// One copy of the editable brush graph plus the bookkeeping needed to
/// invalidate downstream caches when it changes. Shared across all
/// engines spawned from a single `DarklySession`.
pub struct BrushSession {
    /// The brush graph that compiles into a runner on each stroke start.
    /// Defaults to `brush::default_graph()`.
    pub graph: Graph<BrushWireType>,

    /// Snapshot of input port defaults captured the last time the graph
    /// was loaded as a whole (brush load / reset / save). Drives
    /// double-click-to-reset on toolbar scrubs — reset returns to the
    /// brush's shipped value, not the node-type registration default.
    /// Keyed by (node_id, port_name); raw values (not display-space).
    pub defaults: HashMap<(NodeId, String), f32>,

    /// Bumped on every brush-graph mutation (`compile_active`). Used both
    /// as the key the editor preview render is identified by (so stale
    /// readbacks can be discarded) and as a skip predicate — if the
    /// last-rendered version matches, there's nothing to re-render.
    ///
    /// Reflects ALL changes including user-facing scrubs (size, opacity,
    /// …) so the editor and hover previews update as the user adjusts.
    pub version: u64,

    /// Bumped only on changes that affect the brush's *identity* — graph
    /// topology (nodes, wires, exposed flags), node params, and unwired
    /// non-exposed port defaults. User-facing exposed-port scrubs do NOT
    /// bump this version because [`crate::brush::reset_exposed_scrubs`]
    /// neutralises them in the dab-thumbnail render path. The dab cache
    /// keys off this version so resizing the brush leaves the icon alone.
    pub topology_version: u64,
}

impl BrushSession {
    pub fn new() -> Self {
        BrushSession {
            graph: crate::brush::default_graph(),
            defaults: HashMap::new(),
            version: 0,
            topology_version: 0,
        }
    }

    /// Allocate a fresh shared brush session. `DarklySession` (WASM
    /// bridge) calls this once and hands the resulting handle to every
    /// `DarklyEngine` it spawns.
    pub fn shared() -> SharedBrushSession {
        #[allow(clippy::arc_with_non_send_sync)] // see GpuDevice docs
        SharedBrushSession(Arc::new(RwLock::new(BrushSession::new())))
    }
}

impl Default for BrushSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Owning handle to a shared brush session. Cheap to clone (it's just
/// an `Arc`); every clone references the same underlying `BrushSession`.
#[derive(Clone)]
pub struct SharedBrushSession(Arc<RwLock<BrushSession>>);

impl SharedBrushSession {
    /// Take a read guard. The guard must be dropped before any nested
    /// call that might write — but the engine's locking is flat (one
    /// guard per method body) so this is a non-issue in practice.
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, BrushSession> {
        self.0.read().expect("brush session lock poisoned")
    }

    /// Take a write guard.
    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, BrushSession> {
        self.0.write().expect("brush session lock poisoned")
    }
}

impl Default for SharedBrushSession {
    fn default() -> Self {
        BrushSession::shared()
    }
}
