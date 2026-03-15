# GPU-Authoritative Architecture

## Status: PLANNING

This document captures the full context behind a fundamental architectural decision: moving from a CPU-authoritative pixel store (with GPU as a display cache) to a GPU-authoritative system where GPU textures are the source of truth and the CPU dirty tile system is removed entirely.

This is novel engineering — no open-source 2D painting app has done this before.

---

## Part 1: How We Got Here

### The Original System

The current architecture is CPU-authoritative:

- **CPU `TileStore<F>`** is the source of truth for pixel data (both raster layers via `TileStore<Rgba>` and masks via `TileStore<AlphaF32>`)
- **GPU `LayerTexture`** is a display cache, rebuilt from dirty tiles each frame via `render_offscreen()`
- **Undo** uses tile-level Arc COW — `begin_transaction()` records pre-write Arc snapshots, rollback restores them and marks tiles dirty for re-upload
- **Brush painting** generates dabs on CPU, composites per-pixel via `Surface::composite()` callback into the CPU tile store
- **Unified paint target** via the `Surface` enum in `paint.rs` — masks and raster layers share a single API; the fork happens once in `make_surface_for_brush()`, never in the stroke engine

This is the same architecture as Krita, GIMP, MyPaint, Aseprite, and Graphite — every open-source 2D painting app is CPU-authoritative. It was believed to be "GPU-centric" because the compositor runs entirely on GPU, but the pixel store — where data lives and is modified — is CPU.

### The Failed GPU Brush Attempt

A plan was created to add GPU brush rendering while keeping the CPU tile store as the source of truth. The idea: GPU renders dabs to the layer texture during stroke, async readback copies pixels back to CPU at stroke end for undo, CPU tile store remains authoritative for everything else.

The implementation failed, but the interesting question is **why**. The post-mortem identified four mistakes:

1. **Raster/mask fork in `brush_stroke_to()`** — separate GPU path for raster layers, CPU path for masks
2. **Bypassed the brush engine** — hardcoded dab parameters instead of routing `StrokeEngine` output to GPU
3. **Plan never addressed masks** — blind spot led directly to the fork
4. **Premature success claim** — pointed to the hardcoded GPU brush as sufficient

### Reassessing the Failure

On closer examination, most of these are implementation bugs, not architectural blockers:

- The raster/mask fork is fixable — the `Surface` abstraction exists, a `GpuPaintTarget` equivalent is straightforward
- Hardcoded brush parameters are fine for a proof of concept
- The plan's mask blind spot is a planning error, not a design flaw

The bolt-on approach (GPU writes during stroke, readback at stroke end) is **architecturally viable**. The state model is simple: pen down = GPU authoritative, pen up = CPU catches up via readback. One async readback per stroke of only the affected tiles (~50-200 tiles at 16KB each, 1-3MB). Not truly bidirectional — the two directions never happen simultaneously.

**This means there's a real choice to make.** We can either:

1. **Fix the bolt-on** — GPU brush rendering with CPU tile store kept intact, readback at stroke end. Minimal rework, proven architecture.
2. **Go full GPU** — remove the CPU tile store entirely, GPU textures are the only pixel data. Novel, eliminates an entire layer of infrastructure, but requires inventing GPU undo.

### The Decision

We're going full GPU.

The CPU dirty tile system works, and the bolt-on would work too. But maintaining two parallel representations of the same pixel data — CPU tiles with dirty tracking and GPU textures that mirror them — is architectural debt. Every operation either writes to CPU and syncs to GPU, or writes to GPU and syncs to CPU. The sync machinery (dirty bitmaps, per-frame upload loops, readback utilities) exists solely to keep two copies in agreement.

The GPU is already doing the expensive work (compositing, filtering, transforms). Making it also the authority for pixel storage means:
- One copy of the data, not two
- No sync, no dirty tracking, no upload loops
- Operations that the GPU already computes for preview become commits by just writing to the real texture
- The entire `TileStore<F>`, `TiledSurface`, `DirtyRegion`, `Surface`, `PaintTarget`, `MaskPaintTarget` infrastructure goes away

---

## Part 2: Prior Art

### Verified from Source Code

Every claim below was verified by cloning the repository and reading the actual source.

**ArmorPaint** (https://github.com/nicbarker/armortools) — the only open-source project confirmed as GPU-authoritative for pixel storage.

- **Pixel storage:** Layers are GPU render targets. `slot_layer.c:53-57`: `raw->texpaint = render_path_create_render_target(t)->_image`. No CPU pixel buffers.
- **Undo:** GPU-to-GPU texture blit into a ring buffer. `history.c:695-731` (`history_copy_to_undo`): renders a fullscreen copy pass from the layer texture into an undo slot texture. Undo/redo swaps GPU texture handles via `slot_layer_swap()` (`slot_layer.c:168`) — no CPU readback for undo.
- **Color pick:** Renders to a 1x1 GPU texture, reads back that single pixel (`render_path_paint.c:182-208`).
- **Limitations:** 3D texture painter with fixed-size mesh textures, not a 2D editor. No flood fill. Full-layer texture copy per undo step (simple but wasteful — a 4K layer is 64MB per entry).

**Krita** — CPU-authoritative. Pixel data in `KisPaintDevice` tiles on CPU. OpenGL textures are a display cache uploaded via `glTexSubImage2D` (`kis_texture_tile.cpp:317`). `KisOpenGLImageTextures` (`kis_opengl_image_textures.h:36`) explicitly describes itself as "a set of OpenGL textures that contains the projection of a KisImage" — a cache.

**MyPaint / libmypaint** — Entirely CPU. `uint16_t *tile_buffer` (`mypaint-fixed-tiled-surface.c:20-21`). No GPU code anywhere in the library.

**Aseprite** — CPU-authoritative. `Image` class (`image.h:31`) is pure CPU. Skia used for display only.

**Graphite** — CPU-authoritative. `Image<P>` stores `data: Vec<P>` on CPU (`image.rs:43`). Brush `blit` function iterates per-pixel in a CPU loop (`brush.rs:84`). GPU textures created only by uploading from CPU.

### Closed-Source (Unverifiable)

**Procreate** — Widely understood to be GPU-native via their "Silica M" Metal engine (since Procreate 4, 2017). Job postings reference "next generation virtual texturing" and "mobile Tile-Based Deferred Rendering GPUs." No source code or architecture documentation available.

**Substance Painter** — GPU-authoritative via Vulkan. Uses sparse virtual textures (since 2018.3). When VRAM is full, unused textures are transferred back to RAM.

### Summary

| Project | Authority | Undo | 2D Canvas |
|---------|-----------|------|-----------|
| ArmorPaint | GPU (verified) | GPU texture ring buffer | No (3D mesh) |
| Procreate | GPU (unverified) | Unknown | Yes |
| Substance Painter | GPU (unverified) | Unknown | No (3D mesh) |
| Krita | CPU (verified) | CPU tile memento | Yes |
| GIMP | CPU (verified) | CPU GEGL | Yes |
| MyPaint | CPU (verified) | CPU | Yes (infinite) |
| Aseprite | CPU (verified) | CPU command | Yes |
| Graphite | CPU (verified) | CPU graph clone | Yes |

**No open-source 2D painting app with a standard canvas has implemented GPU-authoritative storage.** ArmorPaint is the closest but operates on fixed-size 3D mesh textures with full-texture undo copies. A GPU-authoritative 2D editor with efficient undo is novel.

---

## Part 3: What the GPU-Centric System Needs

### The Fundamental Pieces

Only three pieces of new infrastructure are required. Everything else maps to standard GPU render passes.

#### 1. GPU Undo System

The core invention. Every destructive operation snapshots the affected region before writing, entirely on GPU.

The current CPU system uses tile-level Arc COW — cheap because of reference counting, and memory-efficient because only modified 64×64 tiles are snapshotted. The GPU equivalent cannot use reference counting (GPU memory isn't refcounted), so it needs actual pixel copies.

**Approach: `begin_transaction` + scratch texture + optional diff.**

Same pattern as the current CPU system — snapshot before the operation, not after:

1. `begin_transaction()` — copy the affected region of the layer texture to a **shared scratch texture** (one per engine, reused across all operations)
2. Perform the operation on the layer texture
3. At commit: either store the entire pre-snapshotted region as the undo entry, or diff the scratch against the layer texture to find the minimal changed bounding rect and store only that
4. Release the scratch texture for reuse by the next operation
5. Undo entry stores: (layer_id, rect, offset_into_undo_buffer, pre-operation pixels)
6. On undo: copy the saved rect back from the undo buffer to the layer texture

For brush strokes, the affected region is known from dab positions — track a bounding rect as you paint and snapshot that region at pen-down. No diff needed. The diff (compute shader, sub-millisecond) is only useful for operations where the exact affected region isn't known in advance, or to trim the undo entry down to the minimal changed area after the fact.

**Memory:** Undo entries are proportional to the actual changed area, not the layer size. A brush stroke across a 4K canvas that touches a 200×3000 pixel swath stores ~2.4MB (200×3000×4), not 64MB. Comparable to the current tile-based system.

**Scratch texture cost:** One shared texture, not one per layer. Only one layer is edited at a time, so the scratch texture is reused across all operations. Cost is one layer-sized texture total — e.g., 16MB for a 2048×2048 canvas.

#### 2. GPU Paint Target

A uniform abstraction over "a texture you can paint on" — replaces the CPU-side `Surface` / `PaintTarget` / `MaskPaintTarget`. Wraps either an RGBA8 layer texture or an R8 mask texture. Provides operations (composite dab, erase, replace) as GPU render passes with format-appropriate shaders.

The engine never branches on surface type. It gets a `GpuPaintTarget` and calls methods on it — the same pattern as the existing `Surface` enum, just targeting GPU textures instead of CPU tile stores.

#### 3. Readback Utility

A simple on-demand `readback_region(texture, rect) -> Vec<u8>` for the few operations that inherently need CPU pixels: save/export and clipboard copy. Async via `buffer.map_async()`. Not a persistent data structure — a transient operation invoked when needed.

### What Gets Removed

The following CPU-side infrastructure becomes unnecessary:

| Removed | Reason |
|---------|--------|
| `TileStore<F>` | GPU texture is the pixel store |
| `TiledSurface<F>` | No CPU surface needed |
| `DirtyRegion` | No CPU→GPU sync to track |
| `Surface` enum | Replaced by `GpuPaintTarget` |
| `PaintTarget` / `MaskPaintTarget` | Replaced by `GpuPaintTarget` |
| `render_offscreen()` dirty upload loop | No CPU tiles to upload |
| `Memento<F>` (tile-level) | Replaced by GPU undo regions |
| Per-frame tile upload in compositor | Gone — GPU already has the data |

The undo stack structure (`begin_transaction` / `commit` / `rollback`) stays, but mementos reference GPU buffer regions instead of Arc'd tile data.

### How Every Paint Program Feature Maps to GPU

| Feature | GPU Implementation |
|---------|-------------------|
| Brush painting | Render dab quads onto layer texture via render pass |
| Eraser | Same, with subtractive blend state |
| Smudge/blur/sharpen | Shader reads source region, writes to target |
| Clone stamp | Shader samples from offset location |
| Gradient tool | Fullscreen quad with gradient shader |
| Shape tools | Render geometry onto layer texture |
| Flood fill | Compute shader, or readback → CPU fill → upload |
| Color pick | Render to 1x1 target, single-pixel readback |
| Selections | R8 GPU texture, same abstraction as masks |
| Magic wand | Like flood fill — compute shader or readback |
| Filters | Already on GPU |
| Transforms | Already on GPU |
| Layer compositing | Already on GPU |
| Layer merge/flatten | Composite render pass into one texture |
| Canvas resize | Allocate new texture, blit old content |
| Save/export | On-demand full readback |
| Clipboard copy/paste | Readback for copy, upload for paste |

---

## Part 4: Tiling — CPU vs GPU

### Why CPU Tiling Exists

The CPU tile system serves three purposes:
1. **Sparse storage** — only allocate memory for touched regions (HashMap of 64×64 tiles)
2. **COW undo** — Arc reference counting gives near-free snapshots
3. **Granular dirty tracking** — only upload modified tiles to GPU

### Why GPU Doesn't Need Tiles

GPU textures are contiguous allocations. The hardware sampler reads pixels by UV coordinate — it doesn't care about tile boundaries. Tiling a GPU texture would mean every shader that reads pixels needs tile-boundary logic: which tile am I in? sample from that texture. Near an edge? Sample from the neighbor. This is a massive complexity tax that defeats the purpose of GPU rendering.

Operations like liquify, blur, warp — anything that displaces UV coordinates and samples neighboring pixels — require contiguous textures. Tiling would fight the hardware rather than help it.

The performance optimizations that matter on GPU are different:
- **Sparse storage** → not needed. GPU memory is pre-allocated per layer (we're already paying this cost via `LayerTexture`). The GPU texture exists whether or not a region has been painted.
- **COW undo** → replaced by GPU texture region copies. Still granular (copy only the changed bounding rect), but via `copy_texture_to_texture` not Arc cloning.
- **Dirty tracking** → eliminated. No CPU→GPU sync means nothing to track.

### VRAM Cost

Layer textures are already allocated at full canvas size (padded to tile boundaries). This cost is identical in both architectures — the GPU-authoritative system doesn't make it worse. The additional VRAM cost is only for undo entries and the double-buffer scratch texture.

| Canvas | Raster layer (RGBA8) | Mask (R8) |
|--------|---------------------|-----------|
| 1024×1024 | 4 MB | 1 MB |
| 2048×2048 | 16 MB | 4 MB |
| 4096×4096 | 64 MB | 16 MB |

For large canvases (8K+), VRAM pressure is a hardware constraint. Users with large canvas needs can invest in appropriate hardware.

---

## Part 5: Preview and Commit

### The Current System

The current architecture has **no scratch/preview layer** for brush strokes. Pixels are written directly to the layer's real tile store, and the transaction system captures "before" snapshots for undo. Dirty tiles are re-uploaded to GPU each frame, so strokes appear on screen immediately.

For transforms, there IS a separate preview mechanism: `FloatingContent` extracts source tiles into a GPU texture, and the compositor blends the transformed preview on top of the (cleared) layer each frame via a shader. On commit, `rasterize_to_tiles()` performs CPU pixel-by-pixel bilinear-sampled affine rasterization back to the tile store.

This means the GPU preview and the CPU commit are **two separate implementations of the same operation** — the GPU shader computes the correct transformed pixels for display, then the CPU redoes the exact same work pixel-by-pixel for commit.

### The GPU-Centric Approach

In a GPU-authoritative system, **commit IS the preview**.

**Brush strokes:** Dabs are rendered directly onto the layer texture via GPU render passes. The pixels are immediately visible because the compositor reads from the same texture. No upload step. Before the first dab, snapshot the affected region (or keep the double-buffer). On pen-up, nothing extra happens — the layer texture is already correct. Undo snapshots the diff.

**Transforms:** The compositor blends the transformed source texture on top of the layer, exactly as it does now for preview. On commit, a final GPU render pass writes the transformed result directly to the layer texture. The shader that was already computing the correct pixels for preview just writes to a different target. One implementation, not two. No CPU rasterization. No readback.

**Filters:** Same pattern. The filter preview is a GPU render pass. Commit writes the result to the layer texture. Already GPU-native.

This pattern generalizes: any operation that previews on GPU can commit on GPU. The preview shader is the commit shader — it just writes to the layer texture instead of a temporary. The CPU never touches pixels for any of these operations.

---

## Part 6: What the GPU Engine Enables (Future)

The GPU-authoritative engine is a prerequisite for the advanced brush system, not the other way around. The engine must be stable and working with the basic brush before the node graph brush engine is built on top.

### Non-Destructive Stroke Rendering (Future Brush Engine Feature)

Procreate is not a vector program — it is entirely raster. What appears to be "vector editing" of strokes is actually **full raster re-rendering from stored input data**. Before a stroke begins, Procreate snapshots the layer. As the user draws, it records all sensor data (position, pressure, tilt, speed). Each frame, it restores the snapshot and re-renders the entire stroke from the recorded data. The stroke is never incrementally baked to the layer until the user lifts the pen.

This is directly observable in Procreate's Brush Studio, where you can create a stroke, then tweak brush settings (opacity, texture, stabilization, spacing, etc.) and watch the stroke morph in real-time without painting a new one. The stroke data persists as a replayable sequence; only the brush parameters change.

This is the model the future brush engine will follow. It requires the GPU engine's `RegionStore` (for per-frame snapshot restore) and `GpuPaintTarget` (for dab render passes). The architecture described here is designed to support it — but building the brush engine is a separate project that comes after the GPU engine is stable.

### Why the GPU Engine Must Come First

The advanced brush engine needs three capabilities from the engine layer:

1. **GPU region save/restore** — for per-frame snapshot wipe during non-destructive rendering. This is the `RegionStore` from Part 3.
2. **GPU dab compositing** — render textured quads onto layer textures with alpha blending. This is the `GpuPaintTarget` from Part 3.
3. **GPU undo** — the undo snapshot is the pre-stroke layer state, managed entirely on GPU. This is the GPU undo system from Part 3.

All three are part of the GPU engine, not the brush engine. The brush engine will be a consumer of these primitives. Building it on top of CPU tiles would mean building it twice.

### What Non-Destructive Rendering Enables (When Built)

- **Retroactive stabilization** — smooth, taffy-like line drawing where the stroke path reshapes as new input arrives.
- **Live brush parameter editing** — a Brush Studio where the user paints a stroke, then adjusts brush settings and watches the stroke update in real-time.
- **Full brush interactivity** — smudge, wet blending, canvas color sampling. The brush sees the pre-stroke layer pixels plus all dabs rendered before it in the current frame's pass.
- **Undo for free** — the undo snapshot is the pre-stroke layer state, taken once at pen-down.

### Performance Considerations (For Future Reference)

Re-rendering the entire stroke every frame will eventually need optimization for very long strokes. The architecture supports **checkpoint baking** without structural changes:
- Periodically bake older dabs into the snapshot (advance the snapshot to include committed dabs)
- Only re-render dabs after the checkpoint each frame
- Checkpointing is a pure performance optimization layered on top

On GPU: region copy (microseconds) → render N dab quads (microseconds to low milliseconds) → done. The compositor reads the same texture it always does. No upload, no sync, no dirty tracking. This is the strongest practical argument for the GPU-authoritative architecture — the feature that defines the brush system's identity is naturally cheap on GPU and prohibitively expensive on CPU.

---

## Part 7: Challenges and Open Questions

### GPU Undo Buffer Layout

The undo buffer stores variable-sized bounding rects from different operations on different layers. Options:

- **Large atlas texture with rect packing** — standard 2D bin packing. Wastes some space to fragmentation but simple to blit to/from.
- **GPU buffer (raw bytes)** — more flexible sizes, no texture format constraints, but readback to texture requires a copy.
- **Ring buffer of regions** — FIFO eviction of oldest undo entries when space runs out. Matches the expected access pattern (undo is LIFO, old entries are evicted first).

Mixed RGBA8/R8 undo entries (from layers vs masks) add complexity if using a texture atlas. A raw GPU buffer avoids format issues but adds a copy step.

### Flood Fill / Magic Wand

Algorithms with unpredictable, data-dependent traversal patterns. Options:

- **Compute shader with atomics** — exact, but serial dependencies limit parallelism
- **Jump Flooding Algorithm (JFA)** — approximate (rare pixel-level errors), O(log n) passes, well-suited to GPU
- **Readback → CPU fill → upload** — exact, simple, brief stall (~1-2ms for a reasonable region)

For a v1, the readback approach is simplest and proven. GPU compute fill can be explored later if the latency matters.

### Readback Latency

wgpu only supports async readback (`buffer.map_async()`). Operations that need CPU pixels (save, clipboard, flood fill) must handle the async gap. This is a UX concern, not an architectural one — save already implies a brief pause, clipboard copy can be deferred by a frame, flood fill latency is negligible.

### What Happens to `TileStore<F>`?

It goes away as a persistent structure. No CPU-side pixel store, no dirty tracking, no tile-level undo mementos. What remains:

- The undo stack structure (transaction begin/commit/rollback) — but holding GPU region references, not Arc'd tiles
- A transient readback utility for save/export and clipboard
- Possibly a CPU-side cache for read-heavy operations (histogram, color analysis) if needed — but not a parallel pixel store

---

## Part 8: Novelty Assessment

This rework makes darkly the first open-source 2D painting application with a GPU-authoritative pixel store. The specific innovations:

1. **GPU diff-based undo** — compute shader diffing + bounding rect reduction + region-only snapshots. Achieves memory efficiency comparable to CPU tile-level COW without the tile abstraction.
2. **Unified GPU paint target** — single abstraction over RGBA8 and R8 textures for all paint operations, replacing the CPU-side `Surface` enum.
3. **Preview-is-commit architecture** — operations that preview on GPU commit by writing to the layer texture. One shader implementation per operation, not separate preview and commit paths.

The fundamental risk is that this is uncharted territory for a 2D editor. ArmorPaint proves the basic concept (GPU pixel store + GPU undo) works for 3D texture painting, but the 2D-specific concerns (variable canvas sizes, layer masks, flood fill, selections) have no open-source precedent.

The fundamental opportunity is that every open-source 2D painting app has the same architecture (CPU tiles + GPU display cache), and they all hit the same performance ceiling (CPU-bound brush compositing). Breaking through that ceiling requires a different architecture, and this is it.
