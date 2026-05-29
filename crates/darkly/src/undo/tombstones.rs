//! A set of pixel-bearing node ids whose GPU textures must be kept alive
//! while an undo action sits on the stack — and disposed when the action
//! is evicted, *if* the subtree they belong to is currently detached from
//! the document tree.
//!
//! Several undo actions tombstone GPU textures so that flip-flopping undo
//! / redo can resurrect pixels without snapshotting them. The rule each
//! action enforces in `on_evict` has the same shape: *dispose if the
//! subtree the ids belong to is currently detached*. This type encodes
//! that rule once.
//!
//! Each `Tombstones` carries a polarity (`detached_when_applied`) saying
//! whether the subtree is detached on the action's forward side or its
//! undone side. The action passes its own current `applied` flag in and
//! the helper disposes iff the two match.
//!
//! Polarities by action:
//!
//! | Action                               | Polarity             |
//! |--------------------------------------|----------------------|
//! | [`super::DuplicateAction`]           | `false` (detached when undone) |
//! | [`super::LayerRemoveAction`]         | `true`  (detached when applied) |
//! | [`super::BakeLayersAction`] sources  | `true`  (detached when applied) |
//! | [`super::BakeLayersAction`] result   | `false` (detached when undone) |

use crate::gpu::compositor::Compositor;
use crate::layer::LayerId;

#[derive(Debug)]
pub(crate) struct Tombstones {
    ids: Vec<LayerId>,
    detached_when_applied: bool,
}

impl Tombstones {
    pub(crate) fn new(ids: Vec<LayerId>, detached_when_applied: bool) -> Self {
        Self {
            ids,
            detached_when_applied,
        }
    }

    /// Dispose every tombstoned texture iff the owning action's current
    /// `applied` state means the subtree is detached. Drains the id list
    /// so a second call is a no-op.
    pub(crate) fn dispose_if_detached(&mut self, applied: bool, compositor: &mut Compositor) {
        if applied == self.detached_when_applied {
            for id in self.ids.drain(..) {
                compositor.dispose_layer(id);
            }
        }
    }
}
