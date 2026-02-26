# Krita Undo System Architecture

Analysis of Krita's undo/redo system, based on reading the KDE/krita source code.
Relevant to Darkly's own undo design.

## Core Abstraction: Command Pattern

Krita uses the **command pattern** via `KUndo2Command`, derived from Qt's `QUndoCommand` but heavily extended.

```
libs/command/kundo2stack.h
```

```cpp
class KUndo2Command {
    virtual void undo();
    virtual void redo();
    virtual int id() const;               // for merge identification (-1 = no merging)
    virtual bool mergeWith(const KUndo2Command *other);
    virtual bool timedMergeWith(KUndo2Command *other);  // time-based cumulative merging
    virtual bool canAnnihilateWith(const KUndo2Command *other) const;  // mutual cancellation
    int childCount() const;               // macro/composite support
};
```

Key extensions beyond Qt:
- **`timedMergeWith()`** -- time-based merging for cumulative undo (coalescing old strokes)
- **`canAnnihilateWith()`** -- two commands that cancel each other out can both be removed
- **Time tracking** (`m_timeOfCreation`, `m_endOfCommand`) -- drives the cumulative undo algorithm

One `KUndo2Stack` per document. Ownership chain:
`KisDocument` -> `KisDocumentUndoStore` -> `KUndo2Stack`

---

## Paint Undo: Tile-Level Memento System

Paint strokes do NOT snapshot entire images or re-execute paint operations.
They use a **tile-level memento system** built on copy-on-write (COW).

### The Transaction

```
libs/image/kis_transaction.h
```

`KisTransaction` is a RAII wrapper:
1. Calls `dataManager->getMemento()` to start recording tile changes
2. Painting happens (modifying tiles through the paint device)
3. `endTransaction()` commits the memento

### KisTransactionData -- The Command

```
libs/image/kis_transaction_data.h / .cpp
```

From the source comments:

> "A tile based undo command. Ordinary KUndo2Command subclasses store parameters
> and apply the action in redo(), however, Krita doesn't work like this. Undo
> replaces the current tiles in a paint device with the old tiles, redo replaces
> them again with the new tiles **without actually executing the command that
> changed the image data again**."

- `undo()` calls `dataManager->rollback(memento)` -- swaps current tiles with old
- `redo()` calls `dataManager->rollforward(memento)` -- swaps them back

No re-execution. Pointer swaps on changed tiles only.

### The Memento Manager

```
libs/image/tiles3/kis_memento_manager.h / .cc
```

Each paint device's `KisTiledDataManager` has its own `KisMementoManager`.

**Recording** (`registerTileChange` / `registerTileDeleted`):
- When a tile is about to be modified via COW, the **old** tile data reference is captured in a `KisMementoItem`
- Only one memento item per tile per transaction (subsequent changes to same tile reuse the item)

**Commit** (`commit()`):
- All items in the index are moved to a revision list
- Each item's `parent` pointer links to the previous revision's entry for that tile position
- The tile data pooler is kicked to prepare COW clone copies

**Rollback** (`rollback()`):
- For each item in the revision, replaces the current tile in the hash table with the parent (old) tile
- Recording is blocked during rollback to prevent the swap from being itself recorded
- Revision moves to `m_cancelledRevisions` for potential redo

**Rollforward** (`rollforward()`):
- Takes the first cancelled revision, puts newer tiles back, re-commits

### KisMementoItem

```
libs/image/tiles3/kis_memento_item.h
```

Each item stores:
- Pointer to `KisTileData` (the actual pixel blob)
- Column/row position
- Type: `CHANGED` or `DELETED`
- `parent` pointer (previous memento item for this tile position)

The parent chain allows per-tile traversal through revision history.

---

## Strokes and Macros: Many Dabs = One Undo

A full brush stroke is wrapped in a `KisSavedMacroCommand`:

```
libs/image/commands_new/kis_saved_commands.h
```

1. Stroke starts -> `initStrokeCallback()` creates a `KisSavedMacroCommand` via `postExecutionUndoAdapter->createMacro()`
2. Each dab generates a `KisTransactionData` (one memento per dab), added to the macro
3. Stroke ends -> macro is pushed to undo stack as **one command**
4. Ctrl+Z undoes the entire macro (all dabs in the stroke)

On undo, `KisSavedMacroCommand` creates a **new undo stroke** and submits all sub-commands as stroke jobs. Undo/redo goes through the stroke system, maintaining proper threading and update behavior.

---

## How Different Action Types Map to Commands

| Action type | Command class | Mechanism |
|---|---|---|
| **Paint** | `KisTransactionData` | Tile memento rollback/rollforward |
| **Selections** | `KisSelectionTransaction` | Same tile memento (pixel selection IS a paint device) |
| **Vector/shapes** | `KoShapeTransformCommand`, `KoShapeMoveCommand`, etc. | Traditional old/new state storage |
| **Layer add/remove** | `KisImageLayerAddCommand` / `RemoveCommand` | Store node + parent + position |
| **Layer move** | `KisImageLayerMoveCommand` | Store old/new parent + position |
| **Layer properties** | `KisNodeOpacityCommand`, `KisNodeCompositeOpCommand` | Store old/new property values |
| **Transforms** | `TransformStrokeStrategy` | Generates `KisTransactionData` for raster, `KoShapeTransformCommand` for vector |
| **Filters** | `KisProcessingCommand` | `KisTransactionData` capturing before/after tiles |

### Command Hierarchy

```
KUndo2Command (base)
  +-- KisTransactionData (tile-based paint device undo)
  +-- KisImageCommand (base for image-level operations)
  |     +-- KisImageLayerAddCommand
  |     +-- KisImageLayerRemoveCommand
  |     +-- KisImageLayerMoveCommand
  +-- KisNodeCommand (node property changes)
  +-- KisTransactionBasedCommand (paint-then-capture)
  +-- KoShapeTransformCommand (vector transforms)
  +-- KisSavedMacroCommand (stroke macro container)
  +-- KisCommandUtils::AggregateCommand (lazy init)
  +-- KisCommandUtils::FlipFlopCommand (toggles)
  +-- ... many more
```

---

## Cumulative Undo: Automatic Coalescing of Old Strokes

```
libs/command/KisCumulativeUndoData.h
```

Krita automatically merges old strokes together over time. Parameters:

```cpp
int excludeFromMerge {10};       // keep top N strokes individually undoable
int mergeTimeout {5000};          // after 5s, old strokes can merge
int maxGroupSeparation {1000};    // max gap between strokes in same merge group
int maxGroupDuration {5000};      // max total duration of a merged group
```

The algorithm (in `KUndo2QStack::push()`):
- When a new timed command is pushed, the stack looks backward
- The top 10 strokes are left alone
- Older strokes that exceed `mergeTimeout` and are within `maxGroupSeparation` of each other merge via `timedMergeWith()`
- Merged commands are stored in `m_mergeCommandsVector` on the surviving command

Effect: 50 quick strokes don't require 50 Ctrl+Z presses. Recent strokes stay individually undoable; older ones coalesce.

---

## Memory Management

### COW Tiles

```
libs/image/tiles3/kis_tile_data_interface.h
```

Each tile (64x64 pixels) is backed by a `KisTileData` with:
- `m_usersCount` -- how many tiles/mementos share this data
- `m_refCount` -- shared pointer reference count
- `m_mementoFlag` -- held by memento system?

When writing to a shared tile, COW clones the data. The old data remains referenced by the memento item.

### Pre-Clone Pooler

```
libs/image/tiles3/kis_tile_data_pooler.h
```

Background thread pre-allocates tile copies for COW. Each `KisTileData` has a `m_clonesStack` (lockless). The pooler fills this so COW cloning grabs pre-allocated copies instead of fresh allocations. Kicked after every memento commit.

### Swap to Disk

```
libs/image/tiles3/swap/kis_tile_data_swapper.h
```

Tile data only held by undo history ("historical" -- `mementoed() && numUsers() <= 1`) can be swapped to disk when RAM is tight. A `QReadWriteLock` per tile data coordinates access between render threads and the swapper.

### Memory Stats

`KisTileDataStore::MemoryStatistics` tracks:
- `totalMemorySize` -- all tile data in memory
- `realMemorySize` -- actively used tile data
- `historicalMemorySize` -- only held by undo history
- `poolSize` -- pre-cloned pool
- `swapSize` -- swapped to disk

### Purge

When `KUndo2Stack::m_undo_limit` is exceeded, oldest commands are destroyed. Their `KisTransactionData` destructors drop `KisMementoSP` references, triggering `KisMementoManager::purgeHistory()`, which releases old tile data.

---

## Full Flow: Painting a Stroke

1. **GUI thread**: User starts painting. `FreehandStrokeStrategy` is started via `image->startStroke(strategy)`
2. **Stroke init**: `initStrokeCallback()` creates a `KisSavedMacroCommand` via `postExecutionUndoAdapter->createMacro()`
3. **Each dab**:
   - Creates `KisTransaction` on the target paint device
   - Calls `dataManager->getMemento()` -- starts recording tile changes
   - Painting modifies tiles (COW triggers -- old tile data captured by memento items)
   - `transaction.commit()` commits the memento, adds `KisTransactionData` to the macro
4. **Stroke finish**: `finishStrokeCallback()` pushes the `KisSavedMacroCommand` to the undo stack
5. **On Undo**: `KisSavedMacroCommand::undo()` creates a new undo stroke, submits all sub-commands in reverse. Each `KisTransactionData::undo()` calls `rollback()`, swapping tiles.
6. **Memory**: Pooler pre-allocates clones. Historical tile data can be swapped to disk. Exceeding undo limit purges oldest revisions.

---

## Implications for Darkly

Our current approach snapshots entire `TileGrid`s per stroke. Krita's approach is fundamentally better:

1. **Sparse diffs vs full snapshots** -- Krita only stores the tiles that changed, not the whole grid. Our COW gives us some sharing, but we copy the full grid structure.
2. **Recording vs snapshotting** -- Krita records changes as they happen (memento manager intercepts COW). We take a before-snapshot and hope the diff is implicit via Arc sharing.
3. **Dirty tracking on undo** -- Krita's rollback knows exactly which tiles changed (the memento items). Our current bug: we only mark tiles present in the restored snapshot as dirty, missing tiles that were created by the undone stroke.
4. **Command pattern** -- enables heterogeneous undo (paint, layer ops, transforms all on the same stack). Our snapshot approach only covers raster tile data.
5. **Cumulative undo** -- merging old strokes is a nice UX touch we could adopt later.
