//! GPU undo action for brush strokes painted directly on GPU textures.
//!
//! Stores an `UndoRegionEntry` referencing pixel data in the `RegionStore`'s
//! ring buffer. On undo/redo, the engine swaps the entry via
//! `RegionStore::restore_region` — the action itself just carries the metadata.

use super::UndoAction;
use crate::document::Document;
use crate::gpu::region_store::UndoRegionEntry;
use crate::layer::LayerId;
use std::collections::{HashMap, HashSet};

/// Undo action for GPU paint operations (paint_circle, erase_circle).
///
/// Unlike `TileAction` which stores CPU tile snapshots, this action references
/// pixel data stored in the GPU ring buffer (`RegionStore`). The actual
/// GPU texture restore is executed by the engine, which has access to the
/// device, queue, and compositor textures.
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
}
