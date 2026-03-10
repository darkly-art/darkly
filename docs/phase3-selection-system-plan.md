# Phase 3: Selection System — Implementation Plan

## Context

Phases 1 (AlphaMask + generic tile storage + root group refactor) and 2 (GPU tool overlay with instanced geometry) are complete. Phase 3 adds document-level selections: an `AlphaMask` on the document that gates paint operations, with marching ants visualization and a rectangle select tool. This unlocks Phases 4–6 (more selection tools, layer masks, copy/paste).

### Key architectural clarifications

**Selection masking is CPU-side, and that's correct.** Paint operations (`paint_circle`, `erase_circle`, `flood_fill`, `linear_gradient`) write pixels into CPU-side tiles. The selection must gate these writes — it prevents pixels from being permanently written outside the selection boundary. If we let paint through unmasked and only hid it on the GPU during compositing, the pixels would reappear when you deselect, and undo would be wrong. The selection is a destructive write gate, not a display effect.

**Layer masks (Phase 5) are GPU-side.** Layer masks modulate how a layer composites with layers below it. The compositor already blends layers on the GPU, so layer masks are an additional alpha multiply in the composite shader. Nested groups with masks are handled during the GPU compositor's recursive blend. Phase 3 does not touch layer masks.

**Selection masking overhead is minimal.** The `PaintTarget` abstraction only checks the selection when `doc.selection` is `Some`. When there's no active selection, writes pass through to tiles with zero extra cost. When a selection is active, it's a single `f32` lookup per pixel (`selection.sample(px, py)`) on the one layer being painted — no layer tree traversal, no multi-mask computation.

**Paint tools don't know about masks.** Following Krita's `KisPainter` pattern, selection masking is applied inside the `PaintTarget` write abstraction. Paint methods determine *which* pixels to write and *what* values. `PaintTarget` handles *how* to blend and whether the selection allows the write. This also DRYs up alpha blending logic currently duplicated across four paint methods.

---

## Implementation Steps

### Step 1: Add `selection` field to Document

**File:** `crates/darkly/src/document.rs`

Add `pub selection: Option<AlphaMask>` to `Document`. Initialize as `None`. Import `AlphaMask` from `tile.rs`.

---

### Step 2: PaintTarget — shared pixel write abstraction

**File:** `crates/darkly/src/paint.rs` (new)

Create a `PaintTarget` struct that wraps `(tiles, dirty, selection)` and provides pixel write operations. Selection masking is applied internally — callers never see the selection.

```rust
pub struct PaintTarget<'a> {
    pub tiles: &'a mut TileGrid,
    pub dirty: &'a mut DirtyRegion,
    selection: Option<&'a AlphaMask>,
}

impl<'a> PaintTarget<'a> {
    /// Alpha-composite `src` onto the pixel at (px, py) using normal (over) blending.
    /// Selection mask modulates source alpha automatically.
    pub fn composite(&mut self, px: i32, py: i32, src: [u8; 4]);

    /// Erase (blend toward transparent) at (px, py).
    /// Selection mask modulates erase strength.
    pub fn erase(&mut self, px: i32, py: i32, strength: f32);

    /// Replace pixel at (px, py) with color.
    /// Selection mask modulates via alpha blend (coverage=1 → full replace, 0 → no change).
    pub fn replace(&mut self, px: i32, py: i32, color: [u8; 4]);
}
```

Each method:
1. Computes tile coords and local pixel coords
2. Samples `selection.sample(px, py)` for coverage (1.0 if no selection)
3. Skips write if coverage ≤ 0
4. Modulates the operation's strength by coverage
5. Calls `tiles.get_or_create(tx, ty)` and writes the pixel
6. Marks tile dirty if touched

Register the module in `crates/darkly/src/lib.rs`.

---

### Step 3: Refactor paint methods to use PaintTarget

**File:** `crates/darkly/src/document.rs`

Replace `raster_tiles_and_dirty()` with `paint_target()`:

```rust
fn paint_target(&mut self, layer_id: LayerId) -> Option<PaintTarget<'_>> {
    // Borrow-split: root.children, dirty, and selection are separate fields
    let raster = find_raster_in_mut(&mut self.root.children, layer_id)?;
    let dirty = self.dirty.get_mut(&layer_id)?;
    Some(PaintTarget::new(&mut raster.tiles, dirty, self.selection.as_ref()))
}
```

Refactor each paint method:
- **`paint_circle`** → iterate circle pixels, call `target.composite(px, py, color)`
- **`erase_circle`** → iterate circle pixels, call `target.erase(px, py, 1.0)`
- **`flood_fill`** → walk flood region, call `target.replace(px, py, color)`
- **`linear_gradient`** → iterate all pixels, call `target.replace(px, py, gradient_color)`

Paint methods now only determine which pixels to affect and what values to write.

---

### Step 4: SelectionMode enum + apply

**File:** `crates/darkly/src/document.rs`

```rust
pub enum SelectionMode { Replace, Add, Subtract, Intersect }
```

`Document::apply_selection(shape_mask: AlphaMask, mode: SelectionMode)`:
- **Replace:** `self.selection = Some(shape_mask)`
- **Add:** `selection.boolean_add(&shape_mask)` (or set if None)
- **Subtract:** `selection.boolean_subtract(&shape_mask)` (no-op if None)
- **Intersect:** `selection.boolean_intersect(&shape_mask)` (clear if None)

---

### Step 5: SelectionAction for undo

**File:** `crates/darkly/src/undo/selection.rs` (new)

Snapshot-swap approach — clone the `Option<AlphaMask>` before modification, swap on undo/redo. Cloning is cheap (Arc-based COW tiles — only reference counts change).

```rust
pub struct SelectionAction {
    snapshot: Option<AlphaMask>, // the "other" state, flip-flopped on each undo/redo
}

impl UndoAction for SelectionAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        std::mem::swap(&mut doc.selection, &mut self.snapshot);
        HashMap::new() // no tile dirty marking — engine refreshes marching ants
    }
    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        std::mem::swap(&mut doc.selection, &mut self.snapshot);
        HashMap::new()
    }
}
```

Register in `crates/darkly/src/undo/mod.rs` — add `mod selection; pub use selection::SelectionAction;`.

---

### Step 6: Contour extraction for marching ants

**File:** `crates/darkly/src/mask.rs`

Add `AlphaMask::contour_segments(threshold: f32) -> Vec<([f32; 2], [f32; 2])>`:
- Implements marching squares on the pixel grid
- For each 2×2 block, classify corners as inside (> threshold) or outside (≤ threshold)
- Emit edge segments based on the 16 possible configurations
- Returns line segments in canvas pixel coordinates

The contour is recomputed only when the selection changes (infrequent), not every frame.

---

### Step 7: Engine selection API + overlay merging

**File:** `crates/darkly/src/engine.rs`

Add two fields to `DarklyEngine`:
```rust
selection_overlay: Vec<OverlayPrimitive>,  // marching ants (persistent)
tool_overlay: Vec<OverlayPrimitive>,       // active tool's overlay (transient)
```

**Overlay merging:** Intercept `set_overlay_primitives` and `clear_overlay` — store tool prims in `tool_overlay`. Always push `selection_overlay + tool_overlay` merged to the compositor. This lets marching ants persist while tools show their own overlays on top.

**Selection API:**
- `select_rect(x, y, w, h, mode, antialias, feather)` — rasterize rect SDF into temp `AlphaMask` via `mask.rasterize()`, clone old selection for undo, apply via `doc.apply_selection()`, push `SelectionAction`, update marching ants
- `clear_selection()` — clone old selection, set `doc.selection = None`, push undo, clear ants
- `select_all()` — fill mask covering entire document, push undo
- `invert_selection()` — clone, `selection.invert()`, push undo, update ants
- `has_selection() -> bool`

**Internal helper:** `update_selection_overlay()`:
1. If `doc.selection` is Some → extract contour segments at threshold 0.5
2. Convert segments to `OverlayPrimitive` dashed lines (`KIND_DASHED_LINE`, `FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR`, `dash_len ≈ 8`)
3. Store in `selection_overlay`
4. Push merged overlay to compositor

**Undo/redo hook:** After undo/redo, call `update_selection_overlay()` to refresh marching ants (since `SelectionAction` may have changed the selection).

---

### Step 8: WASM bridge

**File:** `frontend/wasm/src/api.rs`

Add methods to `DarklyHandle`:
- `select_rect(x: f32, y: f32, w: f32, h: f32, mode: &str, antialias: bool, feather: f32)`
- `clear_selection()`
- `select_all()`
- `invert_selection()`
- `has_selection() -> bool`

Mode string mapping: `"replace"` → `Replace`, `"add"` → `Add`, `"subtract"` → `Subtract`, `"intersect"` → `Intersect`.

---

### Step 9: Frontend rectangle select tool

**File:** `frontend/src/tools/rect_select.svelte.ts` (new)

Follows existing tool pattern (see `overlay_debug.svelte.ts`, `brush.svelte.ts`):
- **`onPointerDown`**: record start point in canvas coords
- **`onPointerMove`**: show preview overlay — dashed rect with `FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR`
- **`onPointerUp`**: compute rect bounds from start→end, read modifier keys for mode (none=replace, shift=add, alt=subtract, shift+alt=intersect), call `handle.select_rect(x, y, w, h, mode, true, 0)`, clear preview overlay
- **`onDeactivate`**: clear preview overlay
- **`onKeyDown`**: Escape → `handle.clear_selection()`

Default options: `antialias = true`, `feather = 0`. Options UI deferred to Phase 4.

**File:** `frontend/src/tools/index.ts` — import and register `rectSelectTool`.

---

## Files Summary

| File | Action | What |
|------|--------|------|
| `crates/darkly/src/paint.rs` | Create | `PaintTarget` — shared pixel write abstraction with selection masking |
| `crates/darkly/src/lib.rs` | Modify | Add `pub mod paint;` |
| `crates/darkly/src/document.rs` | Modify | Add `selection` field, `paint_target()`, refactor paint methods, `apply_selection()`, `SelectionMode` |
| `crates/darkly/src/undo/selection.rs` | Create | `SelectionAction` (snapshot swap) |
| `crates/darkly/src/undo/mod.rs` | Modify | Register `SelectionAction` |
| `crates/darkly/src/mask.rs` | Modify | Add `contour_segments()` (marching squares) |
| `crates/darkly/src/engine.rs` | Modify | Selection API, overlay merging, marching ants |
| `frontend/wasm/src/api.rs` | Modify | Selection WASM bridge methods |
| `frontend/src/tools/rect_select.svelte.ts` | Create | Rectangle select frontend tool |
| `frontend/src/tools/index.ts` | Modify | Register rect select tool |

## What's NOT in this phase

- **No Rust-side tool auto-discovery** — tools are frontend-driven (matching existing brush/eraser/fill/gradient pattern). Can revisit in Phase 4 if a pattern emerges.
- **No build.rs changes.**
- **No selection tool options UI** — antialias/feather hardcoded to defaults; options panel in Phase 4.
- **No layer masks** — those are Phase 5 and are GPU-side (composite shader), architecturally separate from selections.

## Verification

1. `cargo build --target wasm32-unknown-unknown` — compiles
2. `cargo test` — existing tests pass + new tests for:
   - `PaintTarget` composite/erase/replace with and without selection
   - `contour_segments` on known mask shapes
   - `SelectionAction` undo/redo round-trip
   - `apply_selection` with all four modes
3. Browser test: activate rect select tool → drag rectangle → marching ants animate → switch to brush → paint across selection boundary → only inside-selection pixels affected → shift+drag to add second rect → Ctrl+Z undoes selection change → marching ants update
