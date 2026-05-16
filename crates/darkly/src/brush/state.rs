//! Brush state shared across every engine in a `DarklySession`.
//!
//! `BrushState` is the brush module's entry in [`crate::tool::ToolSession`]
//! — the generic shared-state bag every tool's cross-engine state goes
//! through. The container has no knowledge of brushes; this file is the
//! brush module's local definition of what it stores there.
//!
//! Access from inside the engine:
//!
//! ```ignore
//! let session = engine.tool_session.read();
//! let brush = session.get::<BrushState>().expect("BrushState registered at session init");
//! // … read brush.graph, brush.version, … …
//! ```
//!
//! Per-engine derived caches (compiled stroke runner, dab pool, preview
//! version cursors) stay on [`crate::engine::DarklyEngine`] — they're
//! rebuildable from this state and don't need to be shared.

use std::collections::HashMap;

use crate::brush::wire::BrushWireType;
use crate::nodegraph::{Graph, NodeId};

/// One copy of the editable brush graph plus the bookkeeping needed to
/// invalidate downstream caches when it changes. Shared across all
/// engines spawned from a single `DarklySession` via `ToolSession`.
pub struct BrushState {
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

impl BrushState {
    pub fn new() -> Self {
        BrushState {
            graph: crate::brush::default_graph(),
            defaults: HashMap::new(),
            version: 0,
            topology_version: 0,
        }
    }
}

impl Default for BrushState {
    fn default() -> Self {
        Self::new()
    }
}
