//! GPU undo action for brush strokes painted directly on GPU textures.
//!
//! Stores an `UndoRegionEntry` whose `EntryPixels` cell owns the action's
//! pixel data — either an in-flight staging buffer (`Pending`) or a
//! host-side `Vec` (`Ready`). On undo/redo, the engine swaps the entry via
//! `RegionScratch::restore_region`; the action itself just carries the
//! metadata and the `Rc<RefCell<EntryPixels>>` handle.

use super::UndoAction;
use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for GPU paint operations (paint, erase, fill, …).
///
/// Owns its pixel data through the entry's `Rc<RefCell<EntryPixels>>` — no
/// shared storage, no ring eviction. The entry's bytes drop when the action
/// drops (`max_steps` overflow, byte-cap eviction, redo cleared by a fresh
/// push, or teardown). GPU texture restore is executed by the engine, which
/// has access to the device, queue, and compositor textures.
pub struct GpuRegionAction {
    entry: UndoRegionEntry,
}

impl GpuRegionAction {
    pub fn new(entry: UndoRegionEntry) -> Self {
        GpuRegionAction { entry }
    }
}

impl UndoAction for GpuRegionAction {
    fn undo(&mut self, _doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // GPU restore is handled by the engine via gpu_region_entry_mut().
        // Return empty map — no CPU tiles to mark dirty.
        HashMap::new()
    }

    fn redo(&mut self, _doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        HashMap::new()
    }

    fn gpu_region_entry_mut(&mut self) -> Option<&mut UndoRegionEntry> {
        Some(&mut self.entry)
    }

    fn byte_cost(&self) -> u64 {
        self.entry.byte_size
    }

    fn on_evict(&mut self, _compositor: &mut Compositor) {
        // Buffer Drop reclaims the staging buffer (Pending) or Vec (Ready)
        // automatically when `self.entry` drops. This override exists to
        // document the lifetime contract: storage is action-owned, no
        // shared pool to release, no extra cleanup required. If a future
        // commit introduces a pool or interns buffers, that release lands
        // here.
    }
}
