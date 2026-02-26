# Phase 1, Session 2 — Layers, Document, Undo, GPU Init

## Scope

Steps 3–4 from the Phase 1 plan. Build the layer types, document model, dirty tracking, and undo stack on top of the tile system from Session 1. Then initialize the GPU context and get a colored clear rendering to the canvas surface.

## Prerequisites

Session 1 complete: project scaffolded, tile system with COW and transaction recording implemented, `cargo test` passes, `npm run start` shows blank canvas.

## Done When

- `cargo test` passes unit tests for layers, document, dirty tracking, paint, and undo/redo
- A colored rectangle clears to the canvas surface in the browser (GPU context working)

---

## Step 3: Layers, document, dirty tracking, undo

### `layer.rs`

```rust
pub type LayerId = u64;

pub enum BlendMode { Normal, Multiply, Screen, Overlay, /* ... */ }

pub struct RasterLayer {
    pub id: LayerId,
    pub tiles: TileGrid,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
}

pub struct FilterLayer {
    pub id: LayerId,
    pub filter: Box<dyn gpu::filter::Filter>,
    pub visible: bool,
}

pub enum Layer {
    Raster(RasterLayer),
    Filter(FilterLayer),
}
```

### `dirty.rs`

```rust
pub struct DirtyRegion {
    pub tiles: HashSet<(i32, i32)>,  // dirty tile coords
}

impl DirtyRegion {
    pub fn mark(&mut self, tx: i32, ty: i32);
    pub fn mark_rect(&mut self, pixel_rect: (u32, u32, u32, u32)); // x,y,w,h
    pub fn clear(&mut self);
    pub fn bounding_rect(&self) -> Option<(i32, i32, i32, i32)>; // tile-coord AABB
}
```

### `document.rs`

```rust
pub struct Document {
    pub layers: Vec<Layer>,         // bottom to top
    pub width: u32,                 // 1920
    pub height: u32,                // 1080
    pub dirty: HashMap<LayerId, DirtyRegion>,
    next_id: LayerId,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self;
    pub fn add_raster_layer(&mut self) -> LayerId;
    pub fn add_filter_layer(&mut self, filter: Box<dyn gpu::filter::Filter>) -> LayerId;
    pub fn paint_circle(&mut self, layer_id: LayerId, cx: f32, cy: f32, radius: f32, color: [u8; 4]);
    pub fn fill_gradient(&mut self, layer_id: LayerId); // demo helper
    pub fn layer(&self, id: LayerId) -> Option<&Layer>;
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer>;
    pub fn layer_index(&self, id: LayerId) -> Option<usize>;
    pub fn begin_transaction(&mut self, layer_id: LayerId);
    pub fn commit_transaction(&mut self, layer_id: LayerId) -> Option<UndoStep>;
}
```

`paint_circle`: iterates tiles touched by the circle's bounding box, calls `tile.write()` on each, sets pixels within radius, marks tiles dirty.

### `undo.rs` — Krita-style tile memento undo

The undo system uses sparse tile mementos, NOT full grid clones. Recording is driven by the frontend (mousedown -> `begin_transaction`, mouseup -> `commit_transaction`), but paint tools are completely unaware — `TileGrid::get_or_create()` transparently captures pre-write state during an active transaction.

```rust
pub struct UndoStep {
    mementos: HashMap<LayerId, Memento>,  // sparse diffs, not full clones
}

pub struct UndoStack {
    undo_steps: Vec<UndoStep>,
    redo_steps: Vec<RedoStep>,
    max_steps: usize,
}

impl UndoStack {
    pub fn push(&mut self, step: UndoStep);  // clears redo history
    pub fn undo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32,i32)>>>;
    pub fn redo(&mut self, doc: &mut Document) -> Option<HashMap<LayerId, HashSet<(i32,i32)>>>;
}

/// Mark affected tiles dirty so the compositor re-uploads them.
pub fn mark_affected_dirty(dirty: &mut HashMap<LayerId, DirtyRegion>,
                           affected: &HashMap<LayerId, HashSet<(i32, i32)>>);
```

`rollback()` and `rollforward()` are symmetric — both swap tile `Arc` pointers and produce the inverse memento for the opposite direction.

**Critical:** `rollback()` may **remove tiles** from the grid (when the memento says `None` — tile didn't exist before the stroke). The GPU compositor must handle this; see Session 3 pitfalls.

### Verification

Unit tests:
- Create document, add layers, paint circles, verify dirty tracking
- Undo/redo round-trip: paint -> commit -> undo -> verify tiles restored -> redo -> verify tiles re-applied
- Semi-transparent dab on empty layer: undo must make the tile blank (not just change alpha)
- Undo on newly created tiles: tiles must be removed from grid, not just zeroed

---

## Step 4: GPU context + surface

### `gpu/context.rs`

```rust
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
}

impl GpuContext {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Self;
    pub fn resize(&mut self, width: u32, height: u32);
    pub fn surface_format(&self) -> wgpu::TextureFormat;
}
```

Init sequence: `Instance::new(Backends::BROWSER_WEBGPU)` -> `instance.create_surface_from_canvas()` -> `instance.request_adapter()` -> `adapter.request_device()` -> configure surface.

### WASM bridge updates

Update the stub `DarklyHandle` from Session 1 to initialize `GpuContext` and render a solid color clear each frame, proving the GPU pipeline works.

### Verification

WASM bridge calls `GpuContext::new()`, surface presents a cleared color each frame. A colored rectangle visible in the browser confirms the GPU context is operational.

---

## Key Reference Files (Graphite, patterns only)

- GPU context: `Graphite/node-graph/libraries/wgpu-executor/src/context.rs`
- WASM entry: `Graphite/frontend/wasm/src/lib.rs`
