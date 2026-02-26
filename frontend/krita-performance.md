# How Krita Achieves High Performance with CPU-Only Painting

> A technical deep-dive into Krita's architecture, written as a reference for
> implementing a photo editor in Rust + WASM + WebGPU.

NOTE: THIS IS NOT A BLUEPRINT FOR HOW TO IMPLEMENT A PERFORMANT EDITOR. IT IS ONLY A REFERENCE CONTAINING POSSIBLE SOLUTIONS TO PERFORMANCE BOTTLENECKS.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [The Tile Engine](#2-the-tile-engine)
3. [Copy-on-Write and the Undo System](#3-copy-on-write-and-the-undo-system)
4. [Projection: Flattening Hundreds of Layers](#4-projection-flattening-hundreds-of-layers)
5. [Dirty Region Tracking](#5-dirty-region-tracking)
6. [The Update Scheduler and Threading Model](#6-the-update-scheduler-and-threading-model)
7. [GPU Texture Streaming](#7-gpu-texture-streaming)
8. [Level-of-Detail (LOD) System](#8-level-of-detail-lod-system)
9. [Memory Management and Swap](#9-memory-management-and-swap)
10. [Putting It All Together: A Brush Stroke](#10-putting-it-all-together-a-brush-stroke)
11. [Design Lessons for a Rust/WASM/WebGPU Editor](#11-design-lessons-for-a-rustwasm-webgpu-editor)

---

## 1. Executive Summary

Krita handles canvases with hundreds of layers at interactive frame rates despite
doing **all** painting and filtering on the CPU. It achieves this through five
interlocking systems:

| System | What it does |
|--------|-------------|
| **Tile engine** | Divides every pixel buffer into 64×64 tiles with lazy allocation and copy-on-write |
| **Dirty region tracking** | When one layer changes, only the affected tiles are recomposited — unchanged regions are skipped entirely |
| **Async update scheduler** | Compositing and stroke jobs run on a thread pool, decoupled from the UI thread |
| **Incremental GPU upload** | Only the tiles that changed are uploaded to the GPU, using a ring of Pixel Buffer Objects to avoid stalls |
| **Level-of-Detail** | When zoomed out, painting and compositing operate on downsampled data |

The critical insight: **Krita never uploads the entire image to the GPU.** It
uploads only the tiles that changed since the last frame, and it composites only
the regions that were dirtied. For a brush stroke affecting a 200×200 pixel area
on one layer in a 300-layer document, the work scales with the stroke size, not
the layer count.

---

## 2. The Tile Engine

> Key files:
> - `libs/image/tiles3/kis_tile_data_interface.h` — tile dimensions and data layout
> - `libs/image/tiles3/kis_tile.h` — tile handle with COW support
> - `libs/image/tiles3/kis_tile_hash_table.h` — spatial hash table
> - `libs/image/tiles3/KisTiledExtentManager.h` — bounding rect tracking

### 2.1 Tile Structure

Every `KisPaintDevice` (the backing store for a layer, mask, or selection)
stores its pixels in a grid of **64×64 pixel tiles**:

```
Tile memory = pixelSize × 64 × 64
            = 16 KB  (8-bit RGBA)
            = 32 KB  (16-bit RGBA)
            = 64 KB  (32-bit float RGBA)
```

Tiles are indexed by `(col, row)` where `col = x / 64`, `row = y / 64`. This
is a logical coordinate, not a pixel coordinate.

### 2.2 Lazy Allocation

Tiles are **not pre-allocated** for the full canvas. The spatial hash table
(`KisTileHashTable`, 1024 buckets with chaining) only contains tiles that have
actually been written to.

When a read is requested for an empty region, `getReadOnlyTileLazy()` returns a
**shared default tile** — a single tile data object filled with the default
pixel (usually transparent). It is never inserted into the hash table.

**Impact**: A 4096×4096 RGBA image that is 90% transparent uses ~1.6 MB instead
of ~64 MB. For a 300-layer document where most layers are mostly empty, this is
the difference between fitting in RAM and not.

### 2.3 The Data Hierarchy

```
KisPaintDevice
  └─ KisTiledDataManager
       └─ KisTileHashTable  (1024-bucket hash map)
            └─ KisTile  (lightweight handle: col, row, lock state)
                 └─ KisTileData  (refcounted pixel buffer, COW-shared)
```

Each `KisTile` is a lightweight handle. The actual pixel buffer lives in
`KisTileData`, which is reference-counted and shared across tiles and undo
history via copy-on-write.

---

## 3. Copy-on-Write and the Undo System

> Key files:
> - `libs/image/tiles3/kis_tile_data.h` — COW refcounting
> - `libs/image/tiles3/kis_memento_manager.h` — undo/redo integration
> - `libs/image/tiles3/kis_tile_data_pooler.h` — clone pre-caching

### 3.1 How COW Works

Each `KisTileData` has two atomic counters:

- **`m_refCount`** — shared pointer semantics (destroy when 0)
- **`m_usersCount`** — number of COW participants (tiles + undo mementos)

When a tile is locked for **reading**, the shared data is used directly. When
locked for **writing**:

1. If `m_usersCount == 1`, the tile owns the data exclusively — write in place.
2. If `m_usersCount > 1`, **clone** the data, swap the pointer, push the old
   data to the undo stack.

### 3.2 Why This Matters for Performance

- **Undo is nearly free for unmodified tiles.** Creating an undo snapshot
  (`commit()`) increments `m_usersCount` on every tile in the layer — but no
  pixel data is copied. Only tiles that are subsequently written to pay the
  copy cost.
- **Layer duplication is O(1).** Duplicating a 300-layer group just increments
  reference counts on existing tile data.

### 3.3 Clone Pre-Caching

A background thread (`KisTileDataPooler`) monitors tiles with high user counts
and **pre-creates clones** stored on a per-tile stack (`m_clonesStack`). When
the next COW write happens, the clone is popped from the stack instead of
allocated and copied on the hot path. This reduces COW latency from ~10–100 µs
(full alloc + memcpy of 16–64 KB) to ~1–2 µs.

---

## 4. Projection: Flattening Hundreds of Layers

> Key files:
> - `libs/image/kis_async_merger.cpp` — the compositing engine
> - `libs/image/kis_base_rects_walker.h` — dirty rect propagation
> - `libs/image/kis_merge_walker.cc` — tree traversal for updates
> - `libs/image/kis_layer_projection_plane.cpp` — per-layer composite interface

### 4.1 What "Projection" Means

Each layer has two paint devices:

- **`original()`** — the raw pixel data the user paints on
- **`projection()`** — the result after applying that layer's effect masks
  (blur, levels, etc.)

Group layers have a projection that is the composite of all their children.
The root image has a final projection representing the entire flattened image.

### 4.2 The Compositing Algorithm

`KisAsyncMerger::startMerge()` processes a **stack of jobs** built by a walker.
For each layer in the stack (bottom to top within a group):

```
1. Setup projection device (allocate or reuse)
2. recalculate() — update this layer's projection (apply effect masks)
3. compositeWithProjection() — bitBlt this layer onto the running composite
4. If topmost in group: writeProjection() to parent's device
```

The key optimization is what `recalculate()` does based on the layer's
**position relative to the dirty layer**:

| Position | Meaning | Action |
|----------|---------|--------|
| `N_FILTHY` | This layer was modified | Full recalculate of the dirty rect |
| `N_FILTHY_PROJECTION` | Effect mask on the modified layer | Recalculate mask output |
| `N_ABOVE_FILTHY` | Above the modified layer | Recalculate only if it depends on lower layers (e.g., adjustment layer) |
| `N_BELOW_FILTHY` | Below the modified layer | **Skip recalculate entirely** — just composite the cached projection |

**This is the core reason hundreds of layers don't tank performance.** When you
paint on layer 150 out of 300, layers 1–149 skip recalculation. Their existing
projection data is composited via `bitBlt`, which for a 200×200 dirty rect
means copying ~160 KB per layer at most — not re-rendering them.

### 4.3 Rect Propagation

Dirty rects can **grow** as they propagate through the layer tree. A blur effect
mask needs extra source pixels beyond the dirty rect (its `needRect` is larger
than its input). The walker computes tight bounds at each step:

- **`changeRect()`** — the output region affected on the parent
- **`needRect()`** — the input region needed from children
- **`accessRect()`** — total data access footprint

These ensure each layer processes the **minimum necessary rectangle**.

---

## 5. Dirty Region Tracking

> Key files:
> - `libs/image/kis_node.cpp` — `setDirty()` entry point
> - `libs/image/kis_simple_update_queue.cpp` — rect merging and splitting
> - `libs/image/KisProjectionUpdateFlags.h` — update flag types

### 5.1 The Dirty Signal Chain

```
User paints 200×200 pixels on Layer 5
    │
    ▼
KisNode::setDirty(QRect(100,100, 200,200))
    │
    ▼
KisNodeGraphListener::requestProjectionUpdate(node, rects, flags)
    │
    ▼
KisUpdateScheduler::updateProjection(node, rects, cropRect, flags)
    │
    ▼
KisSimpleUpdateQueue::addUpdateJob()
    │  ├─ trySplitJob()   — break large rects into patches
    │  └─ tryMergeJob()   — combine adjacent rects to reduce jobs
    │
    ▼
KisMergeWalker created with the dirty rect
    │
    ▼
Walker builds LeafStack (job items with position flags)
    │
    ▼
KisAsyncMerger::startMerge() processes the stack
```

### 5.2 Rect Splitting and Merging

The update queue optimizes batch size:

- **Splitting**: A 2000×2000 dirty rect is broken into smaller patches
  (configurable size) so multiple threads can process them concurrently.
- **Merging**: If two adjacent small rects arrive in quick succession, they are
  merged into one job to reduce overhead.

### 5.3 Graph Sequence Numbers

Each structural change to the layer tree increments a global
`graphSequenceNumber()`. Queued walkers check this on execution — if the tree
changed since the walker was created, it recomputes rather than using stale
traversal data.

---

## 6. The Update Scheduler and Threading Model

> Key files:
> - `libs/image/kis_update_scheduler.h` — central coordinator
> - `libs/image/kis_updater_context.h` — thread pool management
> - `libs/image/kis_update_job_item.h` — per-thread job runner
> - `libs/image/kis_stroke.h` — stroke lifecycle
> - `libs/image/kis_stroke_job_strategy.h` — job sequentiality

### 6.1 Two Queues, One Thread Pool

The scheduler manages two queues:

```
┌─────────────────────┐     ┌──────────────────────┐
│  KisStrokesQueue    │     │ KisSimpleUpdateQueue  │
│  (user actions:     │     │ (projection updates:  │
│   brush dabs,       │     │  compositing dirty    │
│   transforms,       │     │  regions)             │
│   filters)          │     │                       │
└────────┬────────────┘     └────────┬──────────────┘
         │                           │
         ▼                           ▼
    ┌────────────────────────────────────┐
    │       KisUpdaterContext            │
    │  (QThreadPool + job dispatch)     │
    │                                    │
    │  ┌──────┐ ┌──────┐ ┌──────┐      │
    │  │ Job  │ │ Job  │ │ Job  │ ...  │
    │  │ Item │ │ Item │ │ Item │      │
    │  └──────┘ └──────┘ └──────┘      │
    └────────────────────────────────────┘
```

A configurable **balancing ratio** (default 100:1) determines how many stroke
jobs run per update job, ensuring strokes remain responsive.

### 6.2 Job Sequentiality

Stroke jobs declare their concurrency requirements:

| Flag | Behavior |
|------|----------|
| `CONCURRENT` | Multiple jobs run in parallel |
| `SEQUENTIAL` | Waits for all previous jobs in this stroke |
| `BARRIER` | All previous jobs across all strokes must finish |
| `UNIQUELY_CONCURRENT` | Only one instance runs, but concurrent with others |

And their exclusivity:

| Flag | Behavior |
|------|----------|
| `NORMAL` | Takes a read-lock — multiple normal jobs run concurrently |
| `EXCLUSIVE` | Takes a write-lock — blocks all other jobs |

### 6.3 Lock-Free Job Runner

Each `KisUpdateJobItem` runs in a loop with an **atomic state machine**
(`std::atomic<Type>`) to avoid thread wake-up overhead for small jobs:

```
EMPTY → [job assigned] → MERGE/STROKE → [execute] →
    check for more work → EMPTY (exit) or loop back
```

State transitions use `compare_exchange_strong()` (CAS). This means a thread
that finishes one merge job can immediately pick up the next without going
through the thread pool's wake-up path.

### 6.4 Conflict Detection

Before dispatching a merge job, `KisUpdaterContext::isJobAllowed()` checks
whether the new walker's rect overlaps with any currently-running job's rect.
Overlapping jobs are deferred to prevent compositing races on the same pixels.

---

## 7. GPU Texture Streaming

> Key files:
> - `libs/ui/opengl/kis_opengl_image_textures.cpp` — texture management
> - `libs/ui/opengl/kis_texture_tile.cpp` — per-tile GPU texture
> - `libs/ui/opengl/KisOpenGLUpdateInfoBuilder.cpp` — dirty tile list builder
> - `libs/ui/opengl/KisOpenGLBufferCircularStorage.cpp` — PBO ring buffer
> - `libs/ui/opengl/KisOpenGLSync.cpp` — GPU fence synchronization

### 7.1 The Display Is Tiled Too

The GPU-side representation mirrors the CPU tile grid. Each
`KisTextureTile` wraps a single `GL_TEXTURE_2D` corresponding to a region of
the image. The grid is stored as `QVector<KisTextureTile*>` in
`KisOpenGLImageTextures`.

### 7.2 Only Dirty Tiles Are Uploaded

The upload pipeline:

```
KisImage::sigImageUpdated(dirtyRect)
    │
    ▼
KisOpenGLUpdateInfoBuilder::buildUpdateInfo(dirtyRect)
    │  ├─ Calculate which texture tiles intersect dirtyRect
    │  ├─ For each affected tile:
    │  │   ├─ Read pixels from image projection (CPU tile data)
    │  │   ├─ Convert color space if needed
    │  │   └─ Store in pooled DataBuffer
    │  └─ Return KisOpenGLUpdateInfo { tileList, dirtyRect }
    │
    ▼
Update compression (batch multiple dirtyRects)
    │
    ▼
KisOpenGLImageTextures::recalculateCache(updateInfo)
    │
    ▼
For each tile in updateInfo.tileList:
    │
    KisTextureTile::update(tileUpdateInfo)
        ├─ If full tile changed:  glTexImage2D()
        ├─ If partial interior:   glTexSubImage2D(offset, size)
        └─ If touches edge:       build padded buffer, glTexSubImage2D()
```

**For a 200×200 brush stroke**: only ~16 tiles (4×4 grid of 64×64 tiles) are
uploaded. On a 4096×4096 canvas, that's 16 out of 4096 tiles — **0.4%** of the
image.

### 7.3 PBO Ring Buffer (Preventing GPU Stalls)

Uploading textures with `glTexSubImage2D` can stall the CPU if the GPU is still
reading from the previous upload. Krita solves this with a **circular buffer of
Pixel Buffer Objects** (`KisOpenGLBufferCircularStorage`):

```
Buffer pool:  [PBO 0] [PBO 1] [PBO 2] ... [PBO 15]
                                 ▲
                            next upload

1. Bind next PBO in ring
2. Write pixel data to PBO  (CPU → PBO, fast DMA path)
3. glTexSubImage2D()        (PBO → texture, async on GPU)
4. Advance ring pointer
5. Invalidate old PBO data  (glInvalidateBufferData)
```

If the GPU falls behind and the ring wraps around, fence sync
(`glFenceSync` / `glGetSynciv`) detects this and **dynamically grows the buffer
pool** (`allocateMoreBuffers()` doubles the count).

### 7.4 Mipmap Management

Each texture tile maintains mipmaps for smooth zoomed-out rendering. Mipmaps are
regenerated lazily — flagged dirty on update, regenerated on next bind. The
`highq_downscale.frag` shader implements high-quality downsampling by fetching
from multiple mipmap levels.

### 7.5 Update Compression

Multiple `sigImageUpdated` signals arriving within a single frame are compressed
by `KisProjectionUpdatesCompressor` — the dirty rects are unioned, and the GPU
upload happens once for the combined region.

---

## 8. Level-of-Detail (LOD) System

> Key files:
> - `libs/image/KisLodPreferences.h` — LOD configuration
> - `libs/image/kis_lod_transform.h` — coordinate transforms
> - `libs/image/kis_lod_capable_layer_offset.h` — dual-resolution storage
> - `libs/image/kis_lock_free_lod_counter.h` — atomic LOD tracking
> - `libs/image/kis_sync_lod_cache_stroke_strategy.h` — LOD cache generation

### 8.1 The Problem LOD Solves

At 10% zoom on a 4096×4096 image, every pixel on screen represents ~100 source
pixels. Computing a brush dab at full resolution and then downsampling for
display is wasteful.

### 8.2 How It Works

LOD level N means the image is treated as if it were 2^N times smaller:

| LOD | Effective resolution | Tile count (4096² image) |
|-----|---------------------|-------------------------|
| 0   | 4096×4096           | 4096 tiles              |
| 1   | 2048×2048           | 1024 tiles              |
| 2   | 1024×1024           | 256 tiles               |
| 3   | 512×512             | 64 tiles                |

### 8.3 Dual Storage

`KisLodCapableLayerOffset` is a template wrapper that maintains **two copies**
of a value — one for LOD 0 (full res) and one for LOD N:

```cpp
T m_data;        // full resolution
T m_lodNData;    // current LOD level
```

Access is transparent: operators check `currentLevelOfDetail()` and return the
appropriate version.

### 8.4 Stroke LOD Cloning

Strokes can be **cloned** for LOD execution. When painting at LOD 2:

1. The brush engine receives coordinates scaled by `KisLodTransform`
2. It paints into the LOD 2 paint device (1/4 resolution)
3. The compositing runs on LOD 2 data (1/16 the tiles)
4. The display shows the LOD 2 result immediately
5. A background job later regenerates LOD 0 for full-res accuracy

### 8.5 Lock-Free LOD Counter

`KisLockFreeLodCounter` packs a counter and LOD level into a single atomic
integer (counter in upper 24 bits, LOD in lower 8 bits). This lets the scheduler
track how many jobs are running at each LOD level without taking any locks.

---

## 9. Memory Management and Swap

> Key files:
> - `libs/image/tiles3/swap/kis_tile_data_swapper.h` — swap-out policy
> - `libs/image/tiles3/swap/kis_swapped_data_store.h` — swap file I/O
> - `libs/image/tiles3/swap/kis_chunk_allocator.h` — slab allocator
> - `libs/image/tiles3/kis_tile_data_store.h` — global tile registry

### 9.1 Three Tile States

```
NORMAL ──────► COMPRESSED ──────► SWAPPED
(in RAM)       (in RAM, LZF)     (on disk)
   ▲                                 │
   └─────────────────────────────────┘
         (load on access)
```

### 9.2 Swap Policy

A background thread (`KisTileDataSwapper`) monitors total memory usage. When it
exceeds the threshold:

1. **Historical tiles first** — undo data with `mementoFlag == true` and
   `usersCount <= 1` are swapped out before active tiles.
2. **Age-based eviction** — older tiles (not recently accessed) are preferred.
3. **Slab-based disk allocation** — 64 MiB slabs, up to 4 GB total swap file.

### 9.3 Swap Blocking

When a tile is being read or written, it holds a read-lock on `m_swapLock`. The
swapper uses `tryLockForWrite()` — if the lock fails, it skips that tile and
moves on. This ensures swapping never blocks the painting thread.

### 9.4 LZF Compression

Before writing to disk, tile data is compressed with LZF (a fast, lightweight
codec). For images with large areas of similar color, compression ratios of
4–10× are common, meaning the swap file stays small even with extensive undo
history.

---

## 10. Putting It All Together: A Brush Stroke

Here's the complete flow for a single brush dab on layer 150 in a 300-layer,
4096×4096 document:

### Phase 1: Stroke Execution (worker thread)

```
1. Brush engine calculates dab shape (~200×200 pixels)
2. Lock affected tiles for write (3×3 = 9 tiles)
3. COW triggers on tiles shared with undo history
   → Pre-cached clones popped from stack (fast path)
4. Paint dab pixels into tile data
5. Unlock tiles, call setDirty(dabRect)
```

**Tiles touched**: 9 out of ~4096 (0.2%)

### Phase 2: Compositing (worker thread pool)

```
6. Walker builds job stack for dirty rect:
   - Layers 1–149: position = N_BELOW_FILTHY → skip recalculate
   - Layer 150:    position = N_FILTHY → recalculate 200×200 rect
   - Layers 151–300: position = N_ABOVE_FILTHY → recalculate only
     if they depend on lower layers (e.g., adjustment layers)
   - Root: composite all layers for this rect only
7. Each layer's compositeWithProjection() does a bitBlt of the
   200×200 rect from its cached projection onto the running composite
8. Result written to root image projection
```

**Pixels composited per layer**: 200×200 = 40,000 (not 4096×4096 = 16.8M)

### Phase 3: GPU Upload (GL thread)

```
9.  sigImageUpdated(QRect(100,100, 200,200)) emitted
10. Update builder calculates affected GPU tiles (~4×4 = 16 tiles)
11. For each tile:
    - Read 64×64 pixels from projection (CPU)
    - Color-convert if canvas color space differs
    - Bind next PBO from ring buffer
    - memcpy pixel data to PBO
    - glTexSubImage2D from PBO to texture (async)
12. Canvas redraws with updated textures
```

**Bytes uploaded**: 16 tiles × 16 KB = 256 KB (not 64 MB for the full image)

### Total Cost Summary

| Operation | Scale | Approximate cost |
|-----------|-------|-----------------|
| Tile locking + COW | 9 tiles | ~18 µs |
| Dab rasterization | 200×200 px | ~50–200 µs |
| Compositing per layer | 200×200 px | ~10–40 µs × ~300 layers |
| GPU texture upload | 16 tiles, 256 KB | ~100–500 µs |
| **Total** | | **~3–15 ms** (well within 16 ms frame budget) |

---

## 11. Design Lessons for a Rust/WASM/WebGPU Editor

### 11.1 Tile Everything

Use a fixed tile size (64×64 is a good default). This enables:
- Sparse allocation (empty tiles share a default)
- Parallel processing (each tile is independent)
- Incremental GPU upload (only dirty tiles transfer)
- Efficient undo (COW at tile granularity)

In WebGPU, each tile maps to a region within a texture atlas or an individual
texture in an array texture (`texture_2d_array`).

**Graphite does not tile.** It stores vector data as a node graph — there are no
pixel buffers to subdivide. Raster images pass through the pipeline as
whole-image GPU textures. This works because vector data is resolution-
independent and images are typically single layers, not hundreds of large pixel
buffers. A raster editor must tile because its primary data is pixel buffers,
and without tiling every operation touches the full image.

### 11.2 Track Dirty Regions, Not Dirty Layers

The dirty rect, not the dirty layer, determines work volume. A 200×200 stroke
on one layer in a 300-layer document should cost proportional to `200×200 × 300`
(compositing the rect across layers), not `4096×4096 × 300` (full recomposite).

**Graphite does not track dirty regions.** It re-renders the full Vello scene
every frame. This works because Vello is a GPU vector renderer — re-rendering
a scene with a few hundred paths is cheap. A raster editor cannot re-composite
the full image every frame; at 4K with 100+ layers, that's billions of pixel
operations per frame.

### 11.3 Decouple Paint → Composite → Display

Three pipelines with async handoffs:

```
Paint thread(s)  →  Composite thread(s)  →  GPU upload + render
     ↓                     ↓                       ↓
  stroke jobs         merge jobs             texture updates
```

In WASM, you don't have true threads (without SharedArrayBuffer), but you can
use:
- Web Workers for compositing
- `requestAnimationFrame` for GPU upload batching
- `OffscreenCanvas` for parallel rendering

**Graphite does not do this.** It runs all three stages on a single thread in a
`requestAnimationFrame` loop: input events dispatch synchronously, the node
graph evaluates (compositing), and frontend messages flush — all in the same
frame callback. It gets away with this because Vello (GPU) does the heavy
rendering, so the CPU work per frame is minimal.

This won't work for a raster editor. CPU-bound brush rasterization and
per-tile compositing will block the main thread and drop frames. The three-stage
pipeline with workers is necessary.

**What to take from Graphite:** its dispatcher deduplicates idempotent work
(if `RunDocumentGraph` is already queued, duplicates are dropped) and buffers UI
updates until the next frame. Apply the same principle: accumulate dirty rects
from all dabs that arrived since the last frame, merge overlapping regions, and
submit one compositing job for the union.

### 11.4 Never Upload the Full Image

The PBO ring buffer pattern translates directly to WebGPU. This is needed for
the **CPU→GPU path** — uploading dirty tiles from paint strokes to GPU textures
for compositing. Once tiles are on the GPU, compositing and adjustments can stay
GPU-resident (see §11.6), so the staging ring only serves the paint ingest path.

```rust
// WebGPU equivalent of Krita's PBO ring
struct StagingRing {
    buffers: Vec<wgpu::Buffer>,  // MAP_WRITE | COPY_SRC
    next: usize,
}

impl StagingRing {
    fn upload_tile(&mut self, queue: &wgpu::Queue, tile_data: &[u8],
                   texture: &wgpu::Texture, origin: wgpu::Origin3d) {
        let buf = &self.buffers[self.next];
        queue.write_buffer(buf, 0, tile_data);
        // encoder.copy_buffer_to_texture(buf → texture at origin)
        self.next = (self.next + 1) % self.buffers.len();
    }
}
```

**Graphite does not do incremental upload.** Vello builds GPU textures directly
from vector scene data — there is no CPU pixel data to upload. A raster editor
has CPU-originated pixel data (brush strokes painted into tiles), so the staging
ring is necessary for that path. Once tiles are on the GPU, the pipeline should
stay GPU-resident (§11.6).

### 11.5 Use COW for Undo

Rust's `Arc<TileData>` gives you reference counting. Combined with
`Arc::make_mut()` (which clones if refcount > 1), you get COW semantics almost
for free:

```rust
struct Tile {
    data: Arc<TileData>,  // 64×64 pixel buffer
}

impl Tile {
    fn write(&mut self) -> &mut TileData {
        Arc::make_mut(&mut self.data)  // COW: clones only if shared
    }
}
```

Undo snapshots just clone the `Arc` (incrementing the refcount), so creating a
snapshot of a 300-layer document is O(number of tiles) pointer copies, not
O(total pixels).

**Graphite does not use COW for undo.** Its undo system captures node graph
structure (which nodes exist, how they connect, their parameters), not pixel
data. This works because vector documents are small — a node graph with hundreds
of nodes is kilobytes. A raster editor's primary data is pixel buffers, where a
single 4K layer is 32 MB. COW at tile granularity is the only way to make undo
affordable.

### 11.6 Compositing and Adjustments on the GPU

Krita composites on the CPU because it targets OpenGL ES 3.0 (no compute
shaders). With WebGPU, pixel operations can move to the GPU entirely.

**Use render pipelines, not compute shaders.** Run per-pixel operations
(brightness, levels, hue/saturation, blend modes, etc.) as fullscreen triangle
render passes: bind input texture, run fragment shader, output to new texture.
Reserve compute shaders for operations that don't map to texture→texture
transforms (e.g., histogram computation, flood fill, non-local filters).

Render pipelines are preferable because:
- No workgroup size tuning or manual tiling
- Hardware texture sampling is free (filtering, clamping)
- Works identically on all WebGPU implementations
- Simpler code — each adjustment is just a fragment shader

**Chain operations as GPU textures.** Keep data on the GPU through the entire
adjustment and compositing pipeline by chaining render passes: each operation
takes a GPU texture as input and outputs a GPU texture. Every GPU→CPU→GPU
roundtrip costs 5–15ms at 4K, so avoiding readback turns a chain of 10
adjustments from ~100ms of stalls into a few milliseconds of GPU work. Only
read back to CPU at pipeline boundaries (export, saving, CPU-only operations
like brush rasterization).

```
[Layer texture]
    → fragment shader (levels) → GPU texture
    → fragment shader (hue/sat) → GPU texture
    → fragment shader (blend with layer below) → GPU texture
    → ... stays on GPU until display or export
```

**Implement all blend modes in shaders.** Every standard blend mode (normal,
multiply, screen, overlay, etc.) is just color math — a few lines of WGSL per
mode. Reserve CPU fallback for operations that genuinely need it: complex
filters with data-dependent control flow, path boolean operations, or brush
stroke rasterization.

**Compositing strategy:**
- **CPU compositing** (Krita's approach): simpler, good enough for moderate
  layer counts, required when brush strokes originate on CPU
- **GPU compositing** (WebGPU): composite tiles via render passes, keeping all
  data GPU-resident. Eliminates the CPU→GPU upload bottleneck for the
  compositing path, at the cost of GPU memory management
- **Hybrid** (recommended): paint strokes on CPU (tiles, COW undo), upload
  dirty tiles to GPU, composite and apply adjustments entirely on GPU via
  chained render passes

**Graphite does all three of these.** Its adjustment nodes (brightness, levels,
hue/saturation, etc.) each run as a fullscreen triangle render pass with a
fragment shader. Each raster node takes `Raster<GPU>` and outputs `Raster<GPU>`,
keeping the pipeline GPU-resident. All blend modes are implemented as color math
in shaders with no CPU fallback. This is the right approach for a raster editor
too — the same patterns apply directly to tile-based compositing on the GPU.

### 11.7 LOD for Zoomed-Out Views

When the viewport shows the image at <50% zoom, switch to a lower LOD:

- Maintain a mipmap pyramid of the composited projection
- Update only the affected tiles at each LOD level
- Render from the appropriate mip level

WebGPU's `texture.create_view()` with `base_mip_level` makes this
straightforward.

**Graphite does not implement LOD.** It re-renders the full vector scene at
every zoom level via Vello, which is cheap because the work scales with path
count, not pixel count. A raster editor must implement LOD because compositing
scales with pixel count — at 10% zoom on a 4K image, processing full-res data
wastes 100× the necessary work.

### 11.8 Batch GPU→CPU Readback

When readback from GPU to CPU is unavoidable (export, CPU-only filters, saving),
batch all copies into a single GPU submission to amortize command submission
overhead and let the GPU driver schedule all copies together:

1. Create staging buffers for all textures in one pass
2. Encode all `copy_texture_to_buffer` operations in a **single command encoder**
3. Submit once to the queue
4. Map all staging buffers **in parallel** with `futures::try_join_all`
5. Copy from mapped buffers (handling wgpu's 256-byte row alignment padding)

```rust
// Encode ALL copies in one submission
let mut encoder = device.create_command_encoder(&Default::default());
for (texture, staging_buf) in textures.iter().zip(staging_buffers.iter()) {
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::BufferCopyView { buffer: staging_buf, layout, .. },
        texture.size(),
    );
}
queue.submit(std::iter::once(encoder.finish()));

// Map ALL buffers concurrently
futures::try_join_all(
    staging_buffers.iter().map(|buf| buf.slice(..).map_async(MapMode::Read))
).await?;
```

**Graphite does this.** Its `Convert` trait for `Raster<GPU>` → `Raster<CPU>`
encodes all texture-to-buffer copies in one command encoder, submits once, and
maps all staging buffers concurrently. The same pattern applies to a raster
editor — when exporting or saving, batch the readback of all composited tiles
rather than reading them back one at a time.

### 11.9 Memory Pressure and the WASM 4 GB Ceiling

WASM's 32-bit address space limits linear memory to 4 GB. This is the hardest
constraint for a raster editor.

**How far 4 GB goes (per layer, fully painted, RGBA):**

| Resolution | 8-bit | 16-bit |
|---|---|---|
| 1080p | 8 MB (~500 layers) | 16 MB (~250 layers) |
| 4K | 33 MB (~120 layers) | 66 MB (~60 layers) |
| 8K | 132 MB (~30 layers) | 264 MB (~15 layers) |

These are worst-case numbers assuming every layer is fully painted. With tiling
and sparse allocation, most layers are mostly empty — a layer with 20% coverage
uses 20% of the memory. A typical document with 100 layers at 4K 8-bit where
average coverage is 20% uses ~660 MB of tile data, well within budget.

COW undo is nearly free: 50 undo steps modifying ~9 tiles each ≈ 7 MB (tiles
are shared, only cloned on write).

**Where it gets tight:** documents with many fully-painted layers at high bit
depth. 50 fully-painted 4K 16-bit layers = 3.3 GB — dangerously close. And
WASM memory cannot be returned to the OS once allocated, so fragmentation makes
the effective ceiling lower than 4 GB.

**What lives in WASM vs GPU memory matters.** With the hybrid approach from
§11.6, WASM holds tile source data (for painting and undo) while composited
intermediates and display textures live on GPU memory (typically 4–16 GB,
separate address space). This means the 4 GB limit constrains source data and
undo history, not the compositing pipeline.

**Mitigations:**

In a browser, you can't swap to disk like Krita does. Instead:

- **Compress cold tiles in-place** — tiles not touched in the last N seconds
  get LZ4-compressed in WASM memory. Painted regions with smooth gradients
  compress 3–5×, effectively multiplying available memory.
- **Evict to OPFS/IndexedDB** — the browser equivalent of Krita's swap file.
  Move undo history and inactive layer tiles to persistent storage. Slower to
  access (~1–5ms per tile) but removes them from the 4 GB budget entirely.
- **GPU-resident layers** — layers that aren't being painted on (locked,
  hidden, reference layers) can keep tile data only on the GPU, evicting from
  WASM memory. Re-fetch via readback only if the user starts painting on them.
- **Drop oldest undo states** — when memory is tight, discard the oldest undo
  snapshots. COW means only the unique (unshared) tiles are freed.
- **Use `WeakRef` / `FinalizationRegistry`** to detect GC pressure and trigger
  eviction proactively.
- **WASM64** — the eventual fix. 64-bit WASM removes the 4 GB limit entirely,
  but browser support is not yet shipping.

**Graphite does not face memory pressure.** Vector data is tiny — a complex
document is megabytes, not gigabytes. These mitigations are specific to raster
editors.
