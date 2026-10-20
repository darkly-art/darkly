//! Brush state shared across every engine in a `DarklySession`.
//!
//! `BrushState` is the brush module's entry in [`crate::tool::ToolSession`]:
//! the generic shared-state bag every tool's cross-engine state goes
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
//! version cursors) stay on [`crate::engine::DarklyEngine`]; they're
//! rebuildable from this state and don't need to be shared.

use std::collections::HashMap;

use crate::brush::pack::BrushId;
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
    /// double-click-to-reset on toolbar scrubs: reset returns to the
    /// brush's shipped value, not the node-type registration default.
    /// Keyed by (node_id, port_name); raw values (not display-space).
    /// Written by `engine::brush_graph::snapshot_brush_defaults`; read
    /// by the exposed-port query that builds `ExposedPortInfo`.
    pub defaults: HashMap<(NodeId, String), f32>,

    /// Bumped on every brush-graph mutation (`compile_active`). Used both
    /// as the key the editor preview render is identified by (so stale
    /// readbacks can be discarded) and as a skip predicate: if the
    /// last-rendered version matches, there's nothing to re-render.
    ///
    /// Reflects ALL changes including artist-facing scrubs (size, opacity,
    /// …) so the editor and hover previews update as the artist adjusts.
    pub version: u64,

    /// Bumped only on changes that affect the brush's *identity*: graph
    /// topology (nodes, wires, exposed flags), node params, and unwired
    /// non-exposed port defaults. Artist-facing exposed-port scrubs do NOT
    /// bump this version because [`crate::brush::reset_exposed_scrubs`]
    /// neutralises them in the dab-thumbnail render path. The dab cache
    /// keys off this version so resizing the brush leaves the icon alone.
    pub topology_version: u64,

    /// The library brush the live graph was installed from, if any. `None`
    /// after a reset or a YAML import: the graph is nobody's.
    pub loaded: Option<BrushId>,

    /// Unsaved tweaks to brushes that are not loaded, keyed by brush id. The
    /// loaded brush's working copy is `graph` itself, so no brush is ever in
    /// both places. A brush whose live graph equals its saved graph has no
    /// entry: there is nothing to remember.
    ///
    /// Session state: the saved brush in the library is the painter's one
    /// on-disk statement of what a brush is, and `brush_save` is how it
    /// changes, so these never outlive the process.
    pub working_copies: HashMap<BrushId, Graph<BrushWireType>>,
}

impl BrushState {
    pub fn new() -> Self {
        BrushState {
            graph: crate::brush::default_graph(),
            defaults: HashMap::new(),
            version: 0,
            topology_version: 0,
            loaded: None,
            working_copies: HashMap::new(),
        }
    }

    /// Leave the loaded brush: remember the live graph as its working copy if
    /// it differs from `saved`, the brush's graph in the library, and clear
    /// `loaded`. `saved` is `None` when the brush has left the library, which
    /// leaves nothing to stash against.
    pub fn stash_loaded(&mut self, saved: Option<&Graph<BrushWireType>>) {
        let Some(id) = self.loaded.take() else { return };
        if let Some(saved) = saved {
            if self.is_modified(saved) {
                self.working_copies.insert(id, self.graph.clone());
            }
        }
    }

    /// Drop the loaded brush's working copy and clear `loaded`: the painter
    /// threw the tweaks away.
    pub fn discard_loaded(&mut self) {
        if let Some(id) = self.loaded.take() {
            self.working_copies.remove(&id);
        }
    }

    /// Forget brush `id` entirely, as when it is deleted.
    pub fn forget(&mut self, id: &str) {
        self.working_copies.remove(id);
        if self.loaded.as_deref() == Some(id) {
            self.loaded = None;
        }
    }

    /// Whether the live graph differs from `saved`.
    pub fn is_modified(&self, saved: &Graph<BrushWireType>) -> bool {
        *saved != self.graph
    }
}

impl Default for BrushState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state with `id` loaded and its saved graph equal to the live one.
    fn loaded(id: &str) -> (BrushState, Graph<BrushWireType>) {
        let mut state = BrushState::new();
        state.loaded = Some(id.to_string());
        let saved = state.graph.clone();
        (state, saved)
    }

    /// Change one input value on the live graph.
    fn tweak(state: &mut BrushState, value: f32) {
        state
            .graph
            .set_port_default(&NodeId("brush_settings".into()), "size", value)
            .expect("the default graph has brush_settings.size");
    }

    #[test]
    fn stash_only_when_the_graph_differs_from_saved() {
        let (mut state, saved) = loaded("a");
        state.stash_loaded(Some(&saved));
        assert!(
            state.working_copies.is_empty(),
            "an untouched brush leaves no copy"
        );
        assert_eq!(state.loaded, None);

        let (mut state, saved) = loaded("a");
        tweak(&mut state, 0.9);
        state.stash_loaded(Some(&saved));
        assert_eq!(state.working_copies.get("a"), Some(&state.graph));
        assert_eq!(state.loaded, None);
    }

    #[test]
    fn stash_without_a_saved_graph_keeps_nothing() {
        let (mut state, _) = loaded("a");
        tweak(&mut state, 0.9);
        state.stash_loaded(None);
        assert!(state.working_copies.is_empty());
        assert_eq!(state.loaded, None);
    }

    #[test]
    fn discard_loaded_drops_the_copy() {
        let (mut state, _) = loaded("a");
        state.working_copies.insert("a".into(), state.graph.clone());
        state.discard_loaded();
        assert!(state.working_copies.is_empty());
        assert_eq!(state.loaded, None);
    }

    #[test]
    fn forget_clears_loaded_when_it_names_the_id() {
        let (mut state, _) = loaded("a");
        state.working_copies.insert("b".into(), state.graph.clone());
        state.forget("b");
        assert!(state.working_copies.is_empty());
        assert_eq!(
            state.loaded.as_deref(),
            Some("a"),
            "another brush stays loaded"
        );
        state.forget("a");
        assert_eq!(state.loaded, None);
    }

    #[test]
    fn is_modified_compares_structurally() {
        let (mut state, saved) = loaded("a");
        let original = match saved
            .nodes()
            .get(&NodeId("brush_settings".into()))
            .and_then(|n| n.ports.iter().find(|p| p.name == "size"))
        {
            Some(port) => port.value.as_f32(),
            None => panic!("the default graph has brush_settings.size"),
        };
        tweak(&mut state, original + 0.25);
        assert!(state.is_modified(&saved));
        tweak(&mut state, original);
        assert!(!state.is_modified(&saved), "tweaking back is clean");
    }

    #[test]
    fn reordering_exposed_ports_counts_as_modified() {
        let (mut state, saved) = loaded("a");
        assert!(
            state.graph.exposed_ports.len() >= 2,
            "the default graph exposes at least two ports"
        );
        let last = state.graph.exposed_ports.len() - 1;
        let first_key = state.graph.exposed_ports.get_index(0).unwrap().0.clone();
        state.graph.reorder_exposed_port(&first_key, last).unwrap();
        assert!(
            state.is_modified(&saved),
            "brush-bar order is authored state"
        );
    }
}
