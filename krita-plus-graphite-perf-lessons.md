
# Performance recommendations for a RUST+WASM+WEBGPU editor, compiled by comparing performance decisions in both Krita and Graphite

### 11.1 Tile Everything

Use a fixed tile size (64×64 is a good default). This enables:
- Sparse allocation (empty tiles share a default)
- Parallel processing (each tile is independent)
- Incremental GPU upload (only dirty tiles transfer)
- Efficient undo (COW at tile granularity)

In WebGPU, each tile maps to a region within a texture atlas or an individual
texture in an array texture (`texture_2d_array`).

### 11.2 Track Dirty Regions, Not Dirty Layers

The dirty rect, not the dirty layer, determines work volume. A 200×200 stroke
on one layer in a 300-layer document should cost proportional to `200×200 × 300`
(compositing the rect across layers), not `4096×4096 × 300` (full recomposite).

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

### 11.6 Compositing and Adjustments on the GPU

Krita composites on the CPU because it targets OpenGL ES 3.0 (no compute
shaders). With WebGPU, pixel operations can move to the GPU entirely.

**Use render pipelines, not compute shaders.** Run per-pixel operations
(brightness, levels, hue/saturation, blend modes, etc.) as fullscreen triangle
render passes: bind input texture, run fragment shader, output to new texture.
Graphite does this for all its adjustment nodes, and it's the right approach
because:
- No workgroup size tuning or manual tiling
- Hardware texture sampling is free (filtering, clamping)
- Works identically on all WebGPU implementations
- Simpler code — each adjustment is just a fragment shader

Reserve compute shaders for operations that don't map to texture→texture
transforms (e.g., histogram computation, flood fill, non-local filters).

**Chain operations as GPU textures.** Keep data on the GPU through the entire
adjustment and compositing pipeline by chaining render passes: each operation
takes a GPU texture as input and outputs a GPU texture. Graphite does this —
each raster node takes `Raster<GPU>` and outputs `Raster<GPU>` — and it
matters because every GPU→CPU→GPU roundtrip costs 5–15ms at 4K. Avoiding
readback turns a chain of 10 adjustments from ~100ms of stalls into a few
milliseconds of GPU work.

```
[Layer texture]
    → fragment shader (levels) → GPU texture
    → fragment shader (hue/sat) → GPU texture
    → fragment shader (blend with layer below) → GPU texture
    → ... stays on GPU until display or export
```

Only read back to CPU at pipeline boundaries (export, saving, CPU-only
operations like brush rasterization).

**Implement all blend modes in shaders.** Every standard blend mode (normal,
multiply, screen, overlay, etc.) is just color math — a few lines of WGSL per
mode. Graphite does this and none of their blend modes require CPU fallback.
Reserve CPU fallback for operations that genuinely need it: complex filters with
data-dependent control flow, path boolean operations, or brush stroke
rasterization.

**Compositing strategy:**
- **CPU compositing** (Krita's approach): simpler, good enough for moderate
  layer counts, required when brush strokes originate on CPU
- **GPU compositing** (WebGPU): composite tiles via render passes, keeping all
  data GPU-resident. Eliminates the CPU→GPU upload bottleneck for the
  compositing path, at the cost of GPU memory management
- **Hybrid** (recommended): paint strokes on CPU (tiles, COW undo), upload
  dirty tiles to GPU, composite and apply adjustments entirely on GPU via
  chained render passes

### 11.7 LOD for Zoomed-Out Views

When the viewport shows the image at <50% zoom, switch to a lower LOD:

- Maintain a mipmap pyramid of the composited projection
- Update only the affected tiles at each LOD level
- Render from the appropriate mip level

WebGPU's `texture.create_view()` with `base_mip_level` makes this
straightforward.

### 11.8 Batch GPU→CPU Readback

When readback from GPU to CPU is unavoidable (export, CPU-only filters, saving),
batch all copies into a single GPU submission. Graphite does this, and it
matters because it amortizes command submission overhead and lets the GPU driver
schedule all copies together:

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

### 11.9 Memory Pressure

In a browser, you can't swap to disk like Krita does. Instead:
- Use `WeakRef` / `FinalizationRegistry` to detect GC pressure
- Compress undo tile data with a fast codec (LZ4-compatible in WASM)
- Drop the oldest undo states when memory is tight
- Consider IndexedDB for persistent undo storage in large documents
