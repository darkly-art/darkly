use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

use super::UndoAction;

/// Undoable selection change backed by GPU region snapshots.
///
/// Stores two pieces of state:
/// - `was_active`: whether the selection was active *before* this action
/// - `entry`: GPU texture region data for the selection texture
///
/// On undo, `was_active` is swapped with the current active state (stored
/// by the engine before calling undo). The engine reads the restored value
/// via `selection_was_active()` and sets `gpu_selection.active` accordingly.
pub struct SelectionAction {
    was_active: bool,
    entry: UndoRegionEntry,
}

impl SelectionAction {
    pub fn new(was_active: bool, entry: UndoRegionEntry) -> Self {
        SelectionAction { was_active, entry }
    }
}

impl UndoAction for SelectionAction {
    fn undo(&mut self, _doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // No-op on the document — the engine handles gpu_selection.active
        // and GPU texture restore via selection_region_entry_mut().
        HashMap::new()
    }

    fn redo(&mut self, _doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        HashMap::new()
    }

    fn selection_region_entry_mut(&mut self) -> Option<&mut UndoRegionEntry> {
        Some(&mut self.entry)
    }

    fn swap_selection_active(&mut self, current_active: bool) -> Option<bool> {
        let restore_to = self.was_active;
        self.was_active = current_active;
        Some(restore_to)
    }

    fn byte_cost(&self) -> u64 {
        // Same `UndoRegionEntry` byte_size pathway as `GpuRegionAction` —
        // selection-mask snapshots compete for the same WASM heap budget
        // and can easily run to 10s of MB on a complex selection edit.
        self.entry.byte_size
    }

    fn on_evict(&mut self, _compositor: &mut Compositor) {
        // See `GpuRegionAction::on_evict` — storage is action-owned, no
        // shared pool to release. Override exists to document the
        // contract.
    }
}
