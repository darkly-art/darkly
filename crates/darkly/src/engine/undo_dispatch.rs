//! Chokepoint for pushing undo actions through the engine.
//!
//! Every engine-side push of an [`UndoAction`] funnels through here so that
//! actions evicted from the stack (by `max_steps` overflow or by a fresh
//! push clearing the redo history) have their [`UndoAction::on_evict`] hook
//! called with the compositor — releasing any tombstoned GPU textures they
//! own.

use super::DarklyEngine;
use crate::layer::LayerId;
use crate::undo::{CompoundAction, PropertyAction, UndoAction};

impl DarklyEngine {
    /// Push a completed undo action. Runs `on_evict` on every action that
    /// leaves the stack as a result (the cleared redo history plus anything
    /// overflowed past `max_steps`).
    ///
    /// Flushes any pending diff-based undo commit from a just-finished brush
    /// stroke first, so the on-stack action ordering matches the user's
    /// temporal order — a "paint, then duplicate, then undo" sequence first
    /// undoes the duplicate.
    pub(crate) fn push_undo(&mut self, action: Box<dyn UndoAction>) {
        self.flush_pending_undo_commit();
        let evicted = self.undo_stack.push(&mut self.doc, action);
        for mut e in evicted {
            e.on_evict(&mut self.compositor);
        }
    }

    /// Coalesce variant. Mirrors [`Self::push_undo`] for property-change
    /// actions that may merge into the existing top step.
    pub(crate) fn coalesce_property_undo(&mut self, action: PropertyAction) {
        self.flush_pending_undo_commit();
        let evicted = self.undo_stack.coalesce_property(&mut self.doc, action);
        for mut e in evicted {
            e.on_evict(&mut self.compositor);
        }
    }

    /// Run `op` for each id, collecting any returned undo actions into one
    /// [`CompoundAction`] and pushing the bundle as a single undo step. The
    /// closure does the per-id mutation directly (so per-id state can be
    /// snapshotted between iterations — e.g. `position_in_parent` after a
    /// prior id was detached) and returns `Some(action)` if the op produced
    /// any reversible work, `None` if it skipped that id.
    ///
    /// No undo entry is pushed when every id was skipped — keeps the stack
    /// clean when, say, every requested removal hit a locked layer.
    pub(crate) fn batched_undo<F>(&mut self, ids: &[LayerId], mut op: F)
    where
        F: FnMut(&mut Self, LayerId) -> Option<Box<dyn UndoAction>>,
    {
        let mut actions = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(a) = op(self, id) {
                actions.push(a);
            }
        }
        if !actions.is_empty() {
            self.push_undo(Box::new(CompoundAction::new(actions)));
        }
    }

    /// Drain both stacks and run `on_evict` on everything. For document
    /// close / engine teardown so tombstoned textures don't outlive their
    /// owning engine.
    pub(crate) fn drain_undo_for_teardown(&mut self) {
        let actions = self.undo_stack.drain_all();
        for mut a in actions {
            a.on_evict(&mut self.compositor);
        }
    }
}
