# GPU Selection Mask Migration

## Context

Two bugs stem from the selection mask being CPU-only:

1. **Brush strokes ignore the selection.** `brush_stroke_to()` at `painting.rs:234` always uses `default_selection_bind_group` (1×1 white = no masking). The composite shader at bind group slot 2 samples this, so every dab paints as if there's no selection.

2. **Transform/paste adds transparent padding.** `begin_transform()` at `floating.rs:108` computes source bounds from `sel.bounding_rect()`, which returns tile coordinates (multiples of 64), not pixel coordinates. The resulting region is larger than the selection with transparent padding. (`copy_region_from_selection()` in `clipboard.rs` already uses `pixel_bounding_rect()` and is not affected.)

The root cause is that the selection lives in a CPU-side tiled `AlphaMask` (`TileStore<AlphaF32>`) while all painting happens on GPU. The fix is to make the GPU texture the authoritative copy and adapt CPU operations around it.

## Approach: GPU-authoritative selection

The hot path is painting — the brush composite shader samples the selection mask per-pixel for every dab. GPU owns the selection texture. CPU operations adapt:

- **SDF rasterization** — selection tools rasterize on CPU (Rust closures), then upload to the GPU texture. The CPU data is discarded after upload.
- **Boolean ops** — compute/fragment shaders operating directly on the GPU R8 texture. One dispatch per user click.
- **Contour extraction** — async readback of the R8 texture when selection changes (infrequent), then CPU marching squares on the readback data.
- **Undo** — GPU texture copies, same pattern as `GpuRegionAction` already uses for layer undo.
- **Single-pixel sampling** — use the CPU readback cache (populated on every selection change for contour extraction anyway).
- **Copy bounds** — use `ContentBoundsPass` compute shader on the selection texture, or derive from the readback cache.

This eliminates `AlphaMask`/`TileStore` entirely. `doc.selection` becomes a lightweight flag (exists/doesn't exist), and the actual data lives on GPU.

## Phase 0 — Test infrastructure and failing tests

Write failing tests first so we know when the migration succeeds.

### 0a. Make `DarklyEngine` constructible in tests

`GpuContext` currently requires a `wgpu::Surface<'static>` (needs a window). The engine only uses the surface in `render()` and `resize()` — neither is needed for testing painting, selection, or undo. Make `surface` an `Option<wgpu::Surface<'static>>`, bail early in `render()`/`resize()` when headless, and add `GpuContext::new_headless(device, queue)` (gated behind `#[cfg(test)]`). This lets tests construct a real `DarklyEngine` via `test_device()` + `new_headless()`.

### 0b. Engine-level test: brush stroke respects selection

**File:** `crates/darkly/tests/gpu_phase5.rs`

Test through `DarklyEngine`'s public API — the same path real users hit:

1. Construct `DarklyEngine` via headless `GpuContext`
2. `select_rect()` on the left half of the canvas
3. `stroke_to()` with a `BrushStroke` op through the center (spanning both halves)
4. Readback the layer texture
5. Assert: left half (selected) has paint, right half (unselected) is transparent

This test **must fail** before Phase 1d (the one-line fix in `brush_stroke_to()`). The existing `paint_target_selection_masking` test in `gpu_phase1.rs` passes because it goes through `PaintPipelines` directly — it doesn't catch the engine plumbing bug.

### 0c. Engine-level test: transform bounds are tight

Same setup, but:
1. `select_rect()` with a non-tile-aligned rect (e.g., 30×45 at offset 17,23)
2. `begin_transform()`
3. Assert the floating content dimensions match the selection pixel bounds, not tile-aligned bounds

This test **must fail** before Phase 2a.

## Phase 1 — GPU selection texture as source of truth

### 1a. Selection state on DarklyEngine

**File:** `crates/darkly/src/engine/mod.rs`

Replace `doc.selection: Option<AlphaMask>` with a `GpuSelection` struct owning all GPU-side selection state. The engine holds `Option<GpuSelection>` — one optional instead of six that must stay in sync.

```rust
pub struct GpuSelection {
    texture: wgpu::Texture,           // R8Unorm, canvas-sized
    view: wgpu::TextureView,
    brush_bind_group: wgpu::BindGroup,  // for BrushPipelines::selection_bgl
    paint_bind_group: wgpu::BindGroup,  // for PaintPipelines::selection_bind_group_layout
    cpu_cache: Vec<u8>,                 // R8 readback for contour, sampling, bounds
    pixel_bounds: [u32; 4],             // cached tight bounds from cpu_cache
}
```

Methods: `bind_group_for_brush()`, `bind_group_for_paint()`, `pixel_bounds()`, `sample(px, py)`, `invalidate()` (kicks readback + recomputes cache/bounds).

On canvas resize, `GpuSelection` must recreate its texture at the new dimensions, copy existing content, and invalidate all bind groups and the CPU cache.

### 1b. Selection creation from tools

**File:** `crates/darkly/src/engine/selection.rs`

Selection tools still rasterize on CPU (SDF closures), but instead of storing an `AlphaMask`:

1. Rasterize SDF to a flat `Vec<u8>` (R8) directly — skip tile indirection entirely
2. `queue.write_texture()` to the GPU selection texture
3. Create bind groups for both pipeline layouts
4. Kick async readback for CPU cache (contour extraction, bounds)
5. On readback completion: update `selection_cpu_cache`, compute pixel bounds, regenerate marching ants overlay

For **Replace** mode: upload directly.
For **Add/Subtract/Intersect** modes: run a compute/fragment shader combining the new shape with the existing GPU texture.

### 1c. Boolean ops as GPU shaders

**File:** new shader `shaders/selection_combine.wgsl`

Three modes parameterized by a uniform:
- Add: `a + b - a * b` (alpha union — preserves antialiased edges; `min(1.0, a + b)` creates hard edges where two AA selections overlap)
- Subtract: `max(0.0, a - b)`
- Intersect: `min(a, b)`

Input: existing selection texture + newly rasterized shape texture. Output: updated selection texture. Use ping-pong (two textures, swap after each op) — a canvas-sized R8 texture is cheap, and this avoids read-after-write hazards from in-place writes.

### 1d. Fix `brush_stroke_to()`

**File:** `crates/darkly/src/engine/painting.rs:234`

```rust
let sel_bg = self.brush_selection_bind_group.as_ref()
    .unwrap_or(&self.brush_pipelines.default_selection_bind_group);
```

### 1e. Replace ad-hoc upload callers with cached bind group

**File:** `crates/darkly/src/engine/painting.rs` — `gpu_clear_selection()` uses cached `paint_selection_bind_group`.

**File:** `crates/darkly/src/engine/floating.rs` — uses cached bind group instead of `upload_selection_mask()`.

Delete `upload_selection_mask()` and `upload_cropped_selection_mask()`.

### 1f. Undo via GPU texture copies

**File:** `crates/darkly/src/undo/selection.rs`

Replace `SelectionAction { snapshot: Option<AlphaMask> }` with GPU region snapshots, following the existing `GpuRegionAction` pattern:
- Before mutation: copy selection texture region to scratch via `region_store.save_region()`
- On undo/redo: swap texture contents via `region_store.restore_region()`

**Cost note:** Current undo uses Arc-based COW — only modified tiles consume memory (~16KB for a small rect on a 4096² canvas). GPU region snapshots copy the full texture (16MB for 4096² R8). To avoid blowing the region store budget on rapid boolean ops, use `ContentBoundsPass` to compute the dirty region before snapshotting, and only save that subregion.

### 1g. Contour extraction via async readback

**File:** `crates/darkly/src/engine/selection.rs`

After any selection mutation that changes the GPU texture:
1. Kick async readback of full selection texture
2. On completion (next frame): store in `GpuSelection::cpu_cache`, run marching squares, update overlay

This is the same timing as the current `update_selection_overlay()` — just deferred by one frame for the GPU readback.

### 1h. Single-pixel sampling and immediate-correctness operations

`sample(px, py)` reads from `cpu_cache[py * canvas_w + px]`. Used by flood fill seed check.

**Correctness constraint:** The async readback means `cpu_cache` lags one frame behind the GPU texture. This is fine for cosmetic uses (marching ants), but operations that need immediate correctness — copy (ctrl+C), flood fill seed check, `begin_transform()` bounds — must either:
- Wait for the pending readback to complete before proceeding, or
- Do a synchronous readback (blocking) for these specific operations

The simplest approach: track a `cache_valid: bool` flag on `GpuSelection`. Async readback sets it to `true`; any GPU texture mutation sets it to `false`. Operations that need the cache check the flag and block on a synchronous readback if stale.

### 1i. Magic wand adaptation

`select_magic_wand()` currently does async GPU readback of the layer, CPU flood fill, then `AlphaMask::from_r8()`. Post-migration: the flood fill result (`Vec<u8>`) uploads directly to the GPU selection texture via `queue.write_texture()` instead of converting to `AlphaMask`.

## Phase 2 — Fix transform bounds + feathering

### 2a. Fix `begin_transform()` bounds

**File:** `crates/darkly/src/engine/floating.rs:108`

`begin_transform()` currently calls `sel.bounding_rect()` which returns tile coordinates, producing tile-aligned (padded) bounds. Replace with `GpuSelection::pixel_bounds()` for tight pixel-level bounds. (`copy_region_from_selection()` in `clipboard.rs` already uses `pixel_bounding_rect()` and migrates trivially to `GpuSelection::pixel_bounds()`.)

### 2b. Feathering as GPU blur pass

**File:** new shader `shaders/selection_feather.wgsl`

Current feathering is a CPU separable Gaussian blur in `mask.rs`. Phase 3 deletes `AlphaMask`, so feathering must be ported to GPU before that. Implement as a two-pass separable Gaussian blur on the R8 selection texture (horizontal pass → intermediate texture → vertical pass → selection texture). Parameterized by blur radius uniform.

## Phase 3 — Delete AlphaMask/TileStore and tile-aligned GPU padding

Once all consumers are migrated:
- Delete `crates/darkly/src/tile.rs` — no non-selection consumers exist (`Rgba`/`RgbaData` tile format is only used in `tile.rs` unit tests; `TILE_SIZE` is the only export used elsewhere)
- Remove `AlphaMask` type alias and all tile-based methods from `crates/darkly/src/mask.rs`
- Keep `mask.rs` for SDF functions, contour extraction (now operating on flat `Vec<u8>` from readback)
- Remove `TILE_SIZE` padding from `LayerTexture::with_format()` in `gpu/atlas.rs` and `Compositor::new()` in `gpu/compositor.rs` — make textures exactly `canvas_width × canvas_height`. The padding was legacy coupling to the CPU tile grid; all shaders use normalized UVs and the viewport is already set to unpadded canvas dimensions in `paint_target.rs`
- Update `floating.rs` to remove its `TILE_SIZE` import (bounds now come from `GpuSelection::pixel_bounds()`, done in Phase 2a)

## Files to modify

| File | Change |
|------|--------|
| `crates/darkly/src/gpu/context.rs` | Make `surface` optional, add `new_headless()` for tests |
| `crates/darkly/src/engine/rendering.rs` | Bail early in `render()`/`resize()` when headless |
| `crates/darkly/tests/gpu_phase5.rs` | New: engine-level tests for selection+painting and transform bounds |
| `crates/darkly/src/engine/mod.rs` | Add `Option<GpuSelection>`, remove `doc.selection` |
| `crates/darkly/src/engine/selection.rs` | Rewrite selection ops to target GPU texture + readback; adapt magic wand |
| `crates/darkly/src/engine/painting.rs` | Use `GpuSelection::bind_group_for_brush()` in `brush_stroke_to()` + `gpu_clear_selection()` |
| `crates/darkly/src/engine/rendering.rs` | Undo/redo triggers GPU texture swap + readback |
| `crates/darkly/src/engine/floating.rs` | Use `GpuSelection::bind_group_for_paint()` + `pixel_bounds()`; remove `TILE_SIZE` import |
| `crates/darkly/src/engine/clipboard.rs` | Use `GpuSelection::pixel_bounds()` |
| `crates/darkly/src/undo/selection.rs` | GPU region snapshots (dirty-region only) instead of AlphaMask clone |
| `crates/darkly/src/mask.rs` | Contour extraction on flat `Vec<u8>` instead of tiles; remove feathering (moved to GPU) |
| `crates/darkly/src/tile.rs` | Delete |
| `crates/darkly/src/gpu/atlas.rs` | Remove `TILE_SIZE` padding — textures exactly canvas-sized |
| `crates/darkly/src/gpu/compositor.rs` | Remove `TILE_SIZE` padding from accumulator |
| `crates/darkly/src/document.rs` | Remove `selection: Option<AlphaMask>`, add `has_selection: bool` |
| `shaders/selection_combine.wgsl` | New: boolean op shader (add/subtract/intersect, ping-pong) |
| `shaders/selection_feather.wgsl` | New: two-pass separable Gaussian blur |

## Verification

1. Make a rectangular selection, paint with brush → paint clipped to selection
2. Add mode: shift+select a second shape → paint clipped to union (verify antialiased overlap is smooth, not hard-edged)
3. Make a selection, begin transform → floating content has no transparent padding
4. Make a selection, ctrl+c → pasted image has tight bounds
5. Undo/redo a selection, then paint → selection masking still works
6. No selection active → painting works normally (1×1 white fallback)
7. Marching ants appear after selection (one frame delay acceptable)
8. Make a selection, immediately ctrl+c (same frame) → copy uses correct bounds (cache-validity gate works)
9. Feathered selection → smooth falloff (GPU blur matches old CPU Gaussian)
10. Resize canvas with active selection → selection preserved at correct position
11. Magic wand selection → selection appears on GPU, painting is masked
12. `cargo build --target wasm32-unknown-unknown` compiles
