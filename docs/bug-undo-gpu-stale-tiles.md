# Bug: Undo visually broken despite correct CPU-side data

## Status
Fixed. Root cause was in compositor tile upload — see fix below.

## Symptom
Paint a single semi-transparent dab (alpha=200) on an empty layer. Press Ctrl+Z.
Expected: dab disappears. Actual: dab becomes fully opaque (or changes appearance).

## What we built
Replaced the old snapshot-entire-TileGrid undo system with a Krita-style tile memento
system. The new system records only the tiles that change during a stroke:

- `TileGrid::begin_transaction()` starts recording
- `get_or_create()` transparently captures pre-write `Arc<TileData>` into a `Memento`
- `commit_transaction()` returns the memento (sparse diff of changed tiles)
- `rollback()` / `rollforward()` swap tile `Arc` pointers and return affected tile coords
- `UndoStack` stores mementos, not full grid clones

## What's verified working (tests pass)
- Semi-transparent dab on empty layer: undo makes tile blank (`undo_semitransparent_dab_on_empty_layer`)
- Two overlapping strokes: undo restores exact pixel state after stroke 1 (`undo_two_overlapping_strokes`)
- Redo restores exact pixel data
- Memento only records first access per tile per transaction
- New strokes clear redo history

All tests are in `crates/darkly-core/src/undo.rs` and `crates/darkly-core/src/tile.rs`.

## Files involved
- `crates/darkly-core/src/tile.rs` — `TileGrid`, `Memento`, transaction recording
- `crates/darkly-core/src/undo.rs` — `UndoStack`, `UndoStep`, rollback/rollforward
- `crates/darkly-core/src/document.rs` — `begin_transaction()`, `commit_transaction()`
- `frontend/wasm/src/api.rs` — `snapshot(layer_id)`, `commit()`, `undo()`, `redo()`
- `frontend/src/App.svelte` — mousedown calls `snapshot`, mouseup calls `commit`
- `crates/darkly-gpu/src/compositor.rs` — GPU tile upload and compositing
