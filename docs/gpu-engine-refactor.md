# GPU-Authoritative Engine Refactor — Implementation Plan

## Status: PLANNED

Reference: [gpu-authoritative-architecture.md](gpu-authoritative-architecture.md)

### Guiding Principle

The GPU engine is foundational infrastructure. It replaces the CPU tile store as the source of truth for pixel data. The current basic brush (`paint_circle` / `erase_circle` driven by JS input events) must work on the GPU engine before anything else is built on top. The advanced node-graph brush engine is a future project that consumes the GPU engine's primitives — it is not part of this refactor.

Each phase ends with a working, compilable, testable program. No phase leaves the codebase broken.

---

## Current State (master)

The basic brush is simple: JS sends `StrokeOp::PaintCircle { x, y, radius, r, g, b, a }` events. Rust composites them per-pixel onto CPU tiles via `Surface::composite()`. Dirty tiles are uploaded to GPU each frame. Undo uses tile-level Arc COW.

```
JS input event
  → engine.stroke_to(StrokeOp::PaintCircle { ... })
    → doc.paint_circle(layer_id, x, y, radius, color)
      → Surface::Layer(PaintTarget) or Surface::Mask(MaskPaintTarget)
        → per-pixel alpha blend into TileStore
        → mark tile dirty
  → compositor.render_offscreen()
    → scan dirty tiles, upload to GPU textures
    → composite layer tree
```

This is the pipeline we're replacing. The goal is:

```
JS input event
  → engine.stroke_to(StrokeOp::PaintCircle { ... })
    → GpuPaintTarget::composite_circle(layer_tex, x, y, radius, color)
      → GPU render pass directly onto layer texture
  → compositor.render_offscreen()
    → composite layer tree (no upload step)
```

---

## Phase 1: GPU Infrastructure (Additive — No Existing Code Broken)

Build the three new primitives alongside the existing system. Nothing is removed or rewired yet.

### 1A. GPU Region Store

Manages GPU-side undo snapshots: a shared scratch texture and an undo buffer.

**New file:** `crates/darkly/src/gpu/region_store.rs`

```rust
pub struct RegionStore {
    /// Shared scratch texture — holds pre-operation snapshot of the affected region.
    /// One per engine, reused across all operations. Same size as largest layer texture.
    scratch: wgpu::Texture,
    scratch_view: wgpu::TextureView,
    scratch_width: u32,
    scratch_height: u32,

    /// Undo buffer — stores completed undo entries as raw pixel data.
    /// Ring-buffer semantics: oldest entries evicted when space runs out.
    buffer: wgpu::Buffer,
    capacity: u64,
    head: u64,
    entries: VecDeque<UndoRegionEntry>,
}

pub struct UndoRegionEntry {
    layer_id: LayerId,
    rect: [u32; 4],            // x, y, w, h in texture space
    format: wgpu::TextureFormat, // Rgba8Unorm or R8Unorm
    offset: u64,                // byte offset into buffer
    byte_size: u64,
}
```

**Operations:**
- `save_region(encoder, texture, rect)` — copy rect from layer texture to scratch via `copy_texture_to_texture`.
- `commit_region(encoder, texture, rect) -> UndoRegionEntry` — copy the saved scratch region into the undo buffer. Return entry metadata. (Diff shader for minimal bounding rect is optional — skip for v1, use the known rect directly.)
- `restore_region(encoder, entry, texture) -> UndoRegionEntry` — copy saved pixels from buffer back to texture. Returns a forward entry for redo.
- `resize_scratch(device, w, h)` — reallocate when canvas size changes.

**Why a buffer, not a texture atlas:** Mixed RGBA8/R8 entries with variable sizes. Raw bytes avoid format constraints.

### 1B. GPU Paint Target

Abstraction over "a GPU texture you can paint on." Works for both RGBA8 layer textures and R8 mask textures.

**New file:** `crates/darkly/src/gpu/paint_target.rs`

```rust
pub struct GpuPaintTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub format: wgpu::TextureFormat,  // Rgba8Unorm or R8Unorm
    pub width: u32,
    pub height: u32,
}
```

**Operations (each a GPU render pass):**
- `composite_circle(encoder, cx, cy, radius, color, opacity)` — the GPU equivalent of `paint_circle()`. Fragment shader computes SDF from circle center, applies soft edge, alpha-blends onto target.
- `erase_circle(encoder, cx, cy, radius)` — same geometry, subtractive blend state.
- `composite_dab(encoder, dab_texture, position, opacity)` — render a textured quad with alpha blending. (Not used by the basic brush, but this is the primitive the future brush engine will call.)
- `fill_rect(encoder, rect, color)` — solid fill within a rect.
- `clear_rect(encoder, rect)` — clear to transparent/zero.

**Selection masking:** Selection mask (R8 texture) bound as additional input. Shader modulates alpha by selection coverage — same as CPU `PaintTarget::coverage()`.

**Shader:** `shaders/dab_composite.wgsl` — single shader for both RGBA8 and R8 targets. For R8, luminance of input color is used (matching `rgba_to_mask()`). Format distinction handled by pipeline `ColorTargetState`.

### 1C. Readback Utility

On-demand async GPU→CPU pixel readback.

**New file:** `crates/darkly/src/gpu/readback.rs`

```rust
pub struct ReadbackRequest {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}
```

**Operations:**
- `request_readback(device, encoder, texture, rect) -> ReadbackRequest`
- `ReadbackRequest::poll(device) -> Option<Vec<u8>>` — non-blocking.
- `ReadbackRequest::blocking_read(device) -> Vec<u8>` — blocks. For save/export.

**Use cases:** Save/export, clipboard copy, flood fill seed, color picker.

### 1D. What This Phase Does NOT Do

- Does not touch `TileStore`, `DirtyRegion`, `Surface`, `PaintTarget`, or any existing code.
- The new GPU primitives sit alongside the existing system, unused until Phase 2.

---

## Phase 2: Wire Basic Brush Through GPU

Replace the CPU paint path for the basic brush (`paint_circle`, `erase_circle`) with GPU render passes. After this phase, brush strokes write directly to GPU layer textures.

### 2A. New Stroke Flow

Currently `engine.rs:begin_stroke()` calls `doc.begin_transaction()` (CPU tile recording), and `stroke_to()` calls `doc.paint_circle()` (CPU tile compositing).

New flow:

```
begin_stroke(layer_id):
    gpu_target = compositor.get_paint_target(layer_id)  // or mask target
    region_store.save_region(encoder, gpu_target.texture, canvas_rect)
    // no CPU tile transaction

stroke_to(PaintCircle { x, y, radius, color }):
    gpu_target.composite_circle(encoder, x, y, radius, color, 1.0)
    // no CPU tiles touched, no dirty marking
    compositor.needs_composite = true

end_stroke():
    entry = region_store.commit_region(encoder, gpu_target.texture, stroke_rect)
    undo_stack.push(GpuRegionAction::new(entry))
```

### 2B. Stroke Rect Tracking

Track the bounding rect of the stroke as circles are composited. Each `composite_circle` call expands the rect by `(cx - radius, cy - radius, cx + radius, cy + radius)`. This rect is used for `commit_region` — only the changed area is stored in the undo buffer.

### 2C. GPU Undo Action

**New file:** `crates/darkly/src/undo/gpu_region.rs`

```rust
pub struct GpuRegionAction {
    entry: UndoRegionEntry,
}

impl UndoAction for GpuRegionAction {
    fn undo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Swap: restore pre-stroke pixels, capture current as forward entry.
        // GPU writes directly to layer texture — no tile coords needed.
        // Return empty map; compositor.needs_composite set separately.
        HashMap::new()
    }
    fn redo(&mut self, doc: &mut Document) -> HashMap<LayerId, HashSet<(i32, i32)>> {
        // Same swap in reverse.
        HashMap::new()
    }
}
```

Slots into the existing `UndoStack` alongside `LayerAddAction`, `PropertyAction`, etc. The stack is `Vec<Box<dyn UndoAction>>` — it doesn't know or care what's inside.

**Note on undo return type:** `GpuRegionAction` returns an empty map because the GPU texture is already updated by the time `undo()` returns. The compositor just needs `needs_composite = true`. The caller (`engine.rs`) already handles this — `mark_affected_dirty()` is a no-op for an empty map, and the compositor recomposites on the next frame regardless because the texture changed.

### 2D. Compositor Access

The compositor already owns `layer_textures: HashMap<LayerId, LayerTexture>` and `mask_textures: HashMap<LayerId, LayerTexture>`. Add a method to get a `GpuPaintTarget` from these:

```rust
impl Compositor {
    pub fn paint_target(&self, layer_id: LayerId, mask_editing: Option<LayerId>) -> Option<GpuPaintTarget> {
        if mask_editing == Some(layer_id) {
            self.mask_textures.get(&layer_id).map(|t| GpuPaintTarget::from(t))
        } else {
            self.layer_textures.get(&layer_id).map(|t| GpuPaintTarget::from(t))
        }
    }
}
```

### 2E. Coexistence with CPU Path

During Phase 2, only `PaintCircle` and `EraseCircle` stroke ops go through GPU. Other operations (`FloodFill`, `LinearGradient`) still use CPU tiles and the dirty upload loop. The compositor's `render_offscreen()` still runs — it just has fewer dirty tiles to upload because brush strokes no longer dirty them.

Both `TileAction` (CPU) and `GpuRegionAction` (GPU) coexist on the same undo stack. This works because `UndoStack` is polymorphic.

### 2F. What Changes

- `engine.rs:begin_stroke()` — calls `region_store.save_region()` instead of `doc.begin_transaction()`.
- `engine.rs:stroke_to()` — for `PaintCircle`/`EraseCircle`, calls `GpuPaintTarget` methods instead of `doc.paint_circle()` / `doc.erase_circle()`.
- `engine.rs:end_stroke()` — calls `region_store.commit_region()`, pushes `GpuRegionAction` instead of `TileAction`.
- **No changes** to `tile.rs`, `paint.rs`, `document.rs`, `dirty.rs`, or any undo action except the new one.

---

## Phase 3: Migrate Remaining Paint Operations to GPU

Move flood fill, gradient, and clear_selection_contents from CPU tiles to GPU.

### 3A. Gradient → GPU

`linear_gradient()` currently iterates every pixel calling `target.replace()`. Replace with a fullscreen quad render pass:

- Uniforms: start/end point, start/end color, canvas dimensions.
- Fragment shader computes gradient per pixel.
- Renders directly to layer texture via `GpuPaintTarget`.
- Selection masking via bound selection texture.

### 3B. Flood Fill — Hybrid

Keep the CPU algorithm (it's good), change the I/O:

1. `readback_region(layer_tex, region_around_seed)` — read nearby pixels to CPU.
2. Run existing scanline fill on CPU → produces fill mask.
3. Upload fill mask to GPU texture.
4. Render fill mask onto layer texture via GPU render pass (treat fill mask as a large "stamp").

### 3C. clear_selection_contents → GPU

Replace per-pixel iteration with a render pass that clears the layer texture within selection mask bounds, modulated by the selection texture.

### 3D. Color Picker

Read a single pixel from the composited output texture via readback. Or render to a 1×1 target sampling the layer at the pick position.

### 3E. What Changes

- `document.rs` paint methods (`paint_circle`, `erase_circle`, `flood_fill`, `linear_gradient`, `clear_selection_contents`) are no longer called from `engine.rs`. They may be kept temporarily for tests.
- `engine.rs:stroke_to()` — all `StrokeOp` variants now go through GPU.

---

## Phase 4: GPU Transform Commit

Eliminate the double implementation of transforms.

### 4A. Transform Commit as GPU Render Pass

Currently `rasterize_to_tiles()` does per-pixel CPU bilinear sampling. The GPU is already computing the same pixels for preview.

On commit: run the GPU transform shader one final time writing to the layer texture (instead of the compositor's accumulator).

```
commit_transform:
    region_store.save_region(layer_tex, affected_rect)
    clear affected_rect on layer_tex
    render transform shader → layer_tex
    entry = region_store.commit_region(layer_tex, rect)
    undo_stack.push(GpuRegionAction::new(entry))
```

### 4B. FloatingContent Simplification

`FloatingContent` holds `source_tiles: TileGrid` (CPU tiles). After Phase 4, holds `source_texture: wgpu::Texture` instead. Source pixels extracted directly from the existing `LayerTexture` — no CPU tile copy.

### 4C. Paste Path

Clipboard paste uploads pixels directly to a GPU texture via `queue.write_texture()`. Commit writes to layer texture via render pass (identity transform).

### 4D. What's Removed

- `rasterize_to_tiles()` (~70 lines CPU bilinear rasterization).
- `FloatingContent::source_tiles` and CPU tile extraction logic.

---

## Phase 5: Remove CPU Tile Infrastructure

Everything now goes through GPU. Rip out the CPU pixel store.

### 5A. Delete Dead Code

| File | Removed |
|------|---------|
| `tile.rs` | `TileStore<F>`, `TiledSurface<F>`, `Tile<F>`, `Memento<F>`, type aliases |
| `dirty.rs` | `DirtyRegion` entirely |
| `paint.rs` | `PaintTarget`, `MaskPaintTarget`, `Surface` enum, `TransactionMemento`, CPU paint algorithms |
| `undo/tile.rs` | `TileAction` entirely |
| `layer.rs` | `surface: RasterSurface` field → replaced by texture reference |
| `compositor.rs` | Dirty tile upload loop in `render_offscreen()` (~150 lines), staging uploader |
| `document.rs` | `make_surface_for_brush()`, CPU `begin/commit_transaction()`, `has_dirty_layers/masks()`, `clear_all_dirty()` |

### 5B. Layer Ownership Change

Before:
```rust
pub struct RasterLayer {
    pub surface: RasterSurface,      // TiledSurface<Rgba> — CPU tiles + dirty
    pub mask: Option<MaskSurface>,   // TiledSurface<AlphaF32>
}
```

After:
```rust
pub struct RasterLayer {
    pub texture_id: LayerId,         // references compositor's LayerTexture
    pub mask_texture_id: Option<LayerId>,
}
```

The `Compositor`'s `layer_textures` / `mask_textures` are now the source of truth.

### 5C. Save/Export

Without CPU tiles, save uses the readback utility:
1. `readback_region(layer_tex, full_rect) -> Vec<u8>` per layer.
2. Encode pixel data into file format.
3. For large canvases, readback all layers in parallel (one `copy_texture_to_buffer` per layer, single submit).

### 5D. Selection Migration

Move selection mask from `TileStore<AlphaF32>` to an R8 GPU texture. Boolean operations (add/subtract/intersect) use readback → modify → upload — acceptable because selection changes are infrequent.

### 5E. What Survives

- `UndoStack`, `UndoAction` trait, all non-tile undo actions — unchanged.
- `tile.rs` data types (`RgbaData`, `AlphaF32Data`, `TILE_SIZE`) — keep if needed by flood fill CPU path.
- Compositor render passes, blend pipelines, veils — unchanged.
- Affine math helpers — unchanged.

---

## Phase Summary

| Phase | What | Adds | Removes | Risk |
|-------|------|------|---------|------|
| 1. GPU Infrastructure | RegionStore, GpuPaintTarget, Readback | ~600 lines, 3 files, 1 shader | Nothing | Low — purely additive |
| 2. Basic Brush → GPU | Wire paint_circle/erase_circle through GPU | ~300 lines | ~0 (bypass only) | Medium — changes the hot path |
| 3. Remaining Paint Ops | Gradient, flood fill, clear → GPU | ~300 lines | ~200 lines | Low — each op independent |
| 4. Transform Commit | Preview shader = commit shader | ~100 lines | ~150 lines | Low — well-contained |
| 5. Remove CPU Tiles | Delete TileStore, DirtyRegion, Memento | ~0 | ~1500 lines | Medium — many call sites |

---

## Test Plan

### Test Harness: Headless GPU Context

The existing `GpuContext` requires a window surface. Tests need a headless equivalent.

**New file:** `crates/darkly/src/gpu/test_utils.rs` (compiled only under `#[cfg(test)]`)

```rust
/// Create a headless wgpu device + queue for testing.
/// Uses wgpu's automatic backend selection — will use Vulkan, Metal, DX12,
/// or the software fallback depending on the platform.
pub fn test_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,  // headless — no surface needed
        force_fallback_adapter: false,
    })).expect("no GPU adapter available for tests");

    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("test-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    })).expect("failed to create test device")
}

/// Create an RGBA8 texture with known pixel data. Returns texture + view.
pub fn create_test_texture(device: &wgpu::Device, queue: &wgpu::Queue,
                           width: u32, height: u32, data: &[u8]) -> wgpu::Texture { ... }

/// Read back an entire texture to CPU memory (blocking). For test assertions.
pub fn readback_texture(device: &wgpu::Device, queue: &wgpu::Queue,
                        texture: &wgpu::Texture, width: u32, height: u32) -> Vec<u8> { ... }
```

This gives every GPU test a `(device, queue)` pair without a window. CI can run these headlessly (Vulkan on Linux with lavapipe, or wgpu's software fallback).

### Phase 1 Tests

**RegionStore — save/restore round-trip:**
```
1. Create 128×128 RGBA8 texture, fill with known color (red).
2. region_store.save_region(texture, [0, 0, 128, 128]).
3. Overwrite texture with different color (blue).
4. region_store.restore_region(entry, texture).
5. Readback texture → assert all pixels are red again.
```

**RegionStore — partial rect:**
```
1. Create 128×128 texture, fill with red.
2. save_region(texture, [32, 32, 64, 64]) — only inner 64×64 rect.
3. Overwrite entire texture with blue.
4. restore_region(entry, texture).
5. Readback → inner 64×64 is red, outer border is blue.
```

**RegionStore — redo round-trip:**
```
1. Fill texture red. save_region. Fill blue. commit_region → entry_a.
2. restore_region(entry_a) → returns entry_b (forward).
3. Assert texture is red (undo worked).
4. restore_region(entry_b) → returns entry_c.
5. Assert texture is blue (redo worked).
```

**RegionStore — ring buffer eviction:**
```
1. Create store with small capacity (e.g. 128KB).
2. Push undo entries until the buffer wraps.
3. Verify oldest entries are evicted (restore returns error or sentinel).
4. Verify newest entries still work.
```

**RegionStore — R8 format (masks):**
```
1. Same as save/restore round-trip but with R8Unorm texture and single-channel data.
2. Verify format is preserved through save/restore cycle.
```

**GpuPaintTarget — composite_circle:**
```
1. Create 128×128 RGBA8 texture, initially transparent.
2. composite_circle(cx=64, cy=64, radius=10, color=[255,0,0,255], opacity=1.0).
3. Readback → pixel at (64,64) is [255,0,0,255].
4. Pixel at (0,0) is [0,0,0,0] (untouched).
5. Pixel at (64,55) is within the circle — non-transparent.
6. Pixel at (64,50) is outside the circle — transparent.
```

**GpuPaintTarget — alpha blending:**
```
1. Create texture, fill with [0, 0, 255, 128] (semi-transparent blue).
2. composite_circle(cx=64, cy=64, radius=10, color=[255,0,0,128]).
3. Readback center pixel → verify against expected alpha-over blend result.
   Expected: standard Porter-Duff src-over compositing.
```

**GpuPaintTarget — erase_circle:**
```
1. Fill texture with [255, 0, 0, 255].
2. erase_circle(cx=64, cy=64, radius=10).
3. Readback → center pixel alpha is 0 (erased). Border pixels unchanged.
```

**GpuPaintTarget — R8 mask target:**
```
1. Create R8 texture, fill with 255 (fully revealed).
2. composite_circle with black color [0,0,0,255] → luminance 0.0 → paints toward 0.
3. Readback → center pixel is ~0. Border pixels are 255.
```

**GpuPaintTarget — selection masking:**
```
1. Create RGBA8 target, initially transparent.
2. Create R8 selection mask: left half = 255 (selected), right half = 0.
3. composite_circle at center of canvas (spans both halves).
4. Readback → left half has painted pixels, right half is still transparent.
```

**Readback — round-trip:**
```
1. Create texture with known pattern (e.g., row index as red channel).
2. request_readback, blocking_read.
3. Compare returned bytes to input pattern. Exact match.
```

**Readback — sub-rect:**
```
1. Create 128×128 texture with distinct quadrant colors.
2. Readback only top-left 64×64 rect.
3. Verify only the top-left quadrant's pixels are returned.
```

### Phase 2 Tests

**End-to-end GPU brush stroke:**
```
1. Create engine with headless GPU context.
2. begin_stroke(layer_id).
3. stroke_to(PaintCircle { x=50, y=50, radius=5, color=red }).
4. stroke_to(PaintCircle { x=60, y=50, radius=5, color=red }).
5. end_stroke().
6. Readback layer texture → two overlapping circles, alpha-blended.
7. Undo → readback → layer is blank (pre-stroke state).
8. Redo → readback → circles are back, pixel-identical to step 6.
```

**GPU undo interleaved with CPU undo:**
```
1. begin_stroke → paint circles → end_stroke (GPU undo entry).
2. Change layer opacity via PropertyAction (CPU undo entry).
3. Undo → opacity restored (PropertyAction).
4. Undo → painted pixels removed (GpuRegionAction).
5. Redo → pixels back.
6. Redo → opacity changed again.
Verifies mixed TileAction/GpuRegionAction coexistence.
```

**GPU stroke on mask:**
```
1. Add mask to layer, set mask_editing.
2. begin_stroke → paint circles (black) → end_stroke.
3. Readback mask texture → verify painted area is ~0 (hidden).
4. Undo → mask texture is back to 255 (fully revealed).
```

**Stroke rect tracking:**
```
1. Paint circles at known positions.
2. Verify the committed UndoRegionEntry.rect is the tight bounding box
   of all circles (union of cx±radius, cy±radius).
3. This rect should be << canvas size for small strokes.
```

**Multiple strokes with undo:**
```
1. Stroke 1: red circle at (30, 30).
2. Stroke 2: blue circle at (90, 90).
3. Undo → blue circle gone, red circle remains.
4. Undo → both gone.
5. Redo → red back.
6. Redo → blue back.
Pixel-exact comparison at each step.
```

### Phase 3 Tests

**GPU gradient:**
```
1. Render gradient (top-left white, bottom-right black) on layer texture.
2. Readback → verify pixel values follow linear interpolation.
3. Undo → layer is blank.
```

**GPU gradient with selection:**
```
1. Create selection mask covering left half.
2. Render gradient on full canvas.
3. Readback → left half has gradient, right half is transparent.
```

**Flood fill hybrid round-trip:**
```
1. GPU paint a closed red rectangle on a layer.
2. Flood fill inside the rectangle with blue.
3. Readback → interior is blue, exterior is transparent, border is red.
4. Undo → interior is transparent again.
```

**Clear selection contents:**
```
1. Fill entire layer with red.
2. Create selection mask (circle in center).
3. Clear selection contents.
4. Readback → center circle is transparent, surrounding area is red.
```

**Color picker:**
```
1. Paint red at (50, 50), blue at (100, 100).
2. Color pick at (50, 50) → returns [255, 0, 0, 255].
3. Color pick at (100, 100) → returns [0, 0, 255, 255].
4. Color pick at (0, 0) → returns [0, 0, 0, 0] (transparent).
```

### Phase 4 Tests

**Transform commit pixel-accuracy:**
```
1. Paint a small pattern (e.g. 4×4 checkerboard) on a layer.
2. Enter transform mode → translate by (10, 10).
3. Commit.
4. Readback → checkerboard at new position, old position is clear.
5. Undo → checkerboard at original position.
```

**Transform with rotation:**
```
1. Paint a vertical line.
2. Rotate 90 degrees via transform.
3. Commit.
4. Readback → line is now horizontal (verify key pixels).
```

**Paste commit:**
```
1. Upload a small known image as clipboard content.
2. Paste → creates FloatingContent with GPU source texture.
3. Commit.
4. Readback → pasted pixels composited onto layer at paste position.
5. Undo → layer is blank.
```

### Phase 5 Tests

**Regression suite — re-run all Phase 2-4 tests.**
After removing CPU tiles, every previous test must still pass. This is the primary validation that nothing was missed.

**Save/export round-trip:**
```
1. Paint on multiple layers.
2. Export via readback utility.
3. Verify exported pixel data matches what was painted.
```

**Layer add/remove undo still works:**
```
1. Add layer → paint on it → remove layer.
2. Undo remove → layer back with its texture data.
3. Undo paint → layer blank.
4. Undo add → layer gone.
Verifies LayerAddAction/LayerRemoveAction work with GPU textures.
```

**Selection as GPU texture:**
```
1. Create selection (rectangle).
2. Paint with selection active → only selected area receives paint.
3. Modify selection (add circle).
4. Paint again → union area receives paint.
5. Undo selection change → original rectangle restored.
```

---

## What This Enables (After Phase 5)

With the GPU engine stable and the basic brush working on it:

1. **Node-graph brush engine** — builds on `GpuPaintTarget::composite_dab()` and `RegionStore` for non-destructive rendering. The GPU engine provides the primitives; the brush engine is a consumer.
2. **Non-destructive stroke rendering** — per-frame restore-rerender using `RegionStore::save/restore_region()`. Requires the GPU engine but is architecturally separate from it.
3. **GPU compute filters** — filters that read from and write to layer textures directly. Already possible via the existing veil system; destructive layer filters use `GpuPaintTarget` + `RegionStore` for undo.

---

## Open Questions

1. **Undo buffer sizing.** 256MB covers ~16 full-layer undo steps at 4K. Brush strokes touch less area, so 50-100+ undo steps in practice. Make configurable.

2. **Mixed CPU/GPU undo during migration.** Phases 2-4 have both `TileAction` and `GpuRegionAction` on the same stack. Works because `UndoStack` is polymorphic, but test explicitly.

3. **Flood fill GPU (future).** Hybrid readback approach is fine for v1. GPU compute (Jump Flood) can eliminate readback later.

4. **VRAM pressure.** 8K canvas = 256MB per layer. Monitor usage, evict oldest undo entries under pressure.
