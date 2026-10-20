//! Pending edits: what the engine holds outside the undo stack until it is
//! applied, discarded, or resolved ahead of another document edit.
//!
//! Each kind of pending edit is a [`PendingEdit`] implementor living next to
//! its session's code, and [`PENDING_EDITS`] lists them in resolve order.
//! The editor's Apply and Discard, and the frame the canvas draws while a
//! session is open, reach whichever one is presented through that list
//! alone.

use darkly_macros::handlers;

use super::{brush_stroke, floating, shapes, DarklyEngine};

/// How the editor names and draws an apply/discard session: static, because
/// a session kind is a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct EditSessionKind {
    /// Stable id the frontend keys per-kind controls by.
    pub id: &'static str,
    pub display_name: &'static str,
    /// Iconify id.
    pub icon: &'static str,
}

/// An edit the engine holds outside the undo stack until it is applied,
/// discarded, or resolved ahead of another document edit. Each implementor
/// is the session kind; its state lives in its own field on the engine.
pub(crate) trait PendingEdit: Sync {
    fn kind(&self) -> EditSessionKind;
    /// Whether the engine holds this edit now.
    fn held(&self, engine: &DarklyEngine) -> bool;
    /// Whether it is open as a session the artist applies or discards.
    fn presented(&self, engine: &DarklyEngine) -> bool {
        self.held(engine)
    }
    /// Keep the edit's result and end the session.
    fn apply(&self, engine: &mut DarklyEngine);
    /// Drop what the session changed and end it.
    fn discard(&self, engine: &mut DarklyEngine);
    /// Bring the edit to rest before another document edit; `false` while
    /// it is still held (a refused transform commit).
    fn resolve(&self, engine: &mut DarklyEngine) -> bool {
        if !self.held(engine) {
            return true;
        }
        self.apply(engine);
        !self.held(engine)
    }
}

/// Every pending edit the engine can hold, in resolve order. The last
/// stroke comes first: the others push undo entries, and the stroke's
/// entry must precede them. Every session start resolves pending edits, so
/// at most one is held at a time and the order is defensive.
const PENDING_EDITS: &[&dyn PendingEdit] = &[
    &brush_stroke::StrokeEdit,
    &floating::TransformEdit,
    &floating::FloatingEdit,
    &shapes::ShapePlacementEdit,
];

impl DarklyEngine {
    /// Commit any pending edit (the last brush stroke, floating content, a
    /// transform session, a shape placement) before performing other edits.
    /// Call this before operations that would conflict with one (layer
    /// switch, paint, undo, etc.).
    pub fn commit_pending_edits(&mut self) {
        let _ = self.resolve_pending_edits();
    }

    /// Generic chokepoint for document edits that can invalidate a pending
    /// edit: transform topology, capability, selection, target resources, or
    /// a layer that exists in the document but has no undo record yet.
    /// `false` while an edit is still held.
    pub(crate) fn resolve_pending_edits(&mut self) -> bool {
        // A transform still waiting on its setup readback has nothing on
        // screen to apply: it is dropped, and the bumped generation turns
        // the readback stale when it lands.
        if self.pending_transform.take().is_some() {
            self.transform_setup_generation = self.transform_setup_generation.wrapping_add(1);
        }
        PENDING_EDITS.iter().all(|edit| edit.resolve(self))
    }

    /// The edit open as an apply/discard session, if any.
    pub(crate) fn active_edit_session(&self) -> Option<&'static dyn PendingEdit> {
        let mut presented = PENDING_EDITS.iter().copied().filter(|e| e.presented(self));
        let active = presented.next();
        debug_assert!(
            presented.next().is_none(),
            "at most one edit session is presented"
        );
        active
    }
}

#[handlers]
impl DarklyEngine {
    /// Apply the open edit session: keep what it changed and end it.
    /// `false`, changing nothing, when no session is open.
    #[handler]
    pub fn apply_edit_session(&mut self) -> bool {
        let Some(edit) = self.active_edit_session() else {
            return false;
        };
        edit.apply(self);
        true
    }

    /// Discard the open edit session: drop what it changed and end it.
    /// `false`, changing nothing, when no session is open.
    #[handler]
    pub fn discard_edit_session(&mut self) -> bool {
        let Some(edit) = self.active_edit_session() else {
            return false;
        };
        edit.discard(self);
        true
    }
}
