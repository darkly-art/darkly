# Darkly Phase 1 — Layer System + GPU Compositor

## Context

Darkly is a browser-based art tool that uses "Veils" (filter layers) to obscure and transform artwork, stimulating creative exploration. Phase 1 establishes the foundation: a tiled raster layer system with GPU compositing and filter shaders. No UI — just a canvas with hardcoded layers and mouse painting.

The project is a new standalone Rust+WASM+Svelte codebase at `/mega/ARTEXP/darkly/`. The Graphite editor at `/mega/ARTEXP/darkly/Graphite/` is reference only.

**Demo goal:** Two raster layers + a Gaussian blur filter layer between them. Mouse paints on the top layer. The blur filter obscures the painting. The bottom layer has a pre-filled color gradient. This proves tiles, dirty tracking, GPU compositing, blend modes, and filter shaders all work end-to-end.

**Engineering principle:** The core engine does not need to be 100% implemented, but every part that is implemented must be implemented properly on the first iteration. No hacks, no hardcoding, no shortcuts in the engine. If we implement one filter, we build a proper modular filter system and register that one filter in it. If we implement one blend mode, we build a proper blend mode system. The UI/demo layer can hardcode and cut corners freely — the engine cannot. This applies to every system: tiles, layers, filters, compositing, undo, GPU resource management.

---

## Project Structure

```
darkly/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── darkly-core/              # CPU-side: layers, tiles, undo, dirty tracking
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── tile.rs           # TileData, Tile (Arc COW), TileGrid
│   │       ├── layer.rs          # RasterLayer, FilterLayer, Layer enum
│   │       ├── document.rs       # Document: ordered layer stack + operations
│   │       ├── dirty.rs          # DirtyRegion: per-layer dirty tile coords
│   │       └── undo.rs           # COW undo/redo stack
│   │
│   └── darkly-gpu/               # GPU-side: wgpu context, atlas, compositor
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── context.rs        # GpuContext: Device, Queue, Surface init
│           ├── atlas.rs          # TileAtlas: GPU texture storage for tiles
│           ├── staging.rs        # StagingRing: CPU→GPU tile upload buffers
│           ├── compositor.rs     # Compositor: chained render passes
│           ├── blend.rs          # Blend mode pipeline management
│           └── filter.rs         # Filter shader pipeline management
│
├── frontend/
│   ├── package.json
│   ├── vite.config.ts
│   ├── tsconfig.json
│   ├── index.html
│   ├── src/
│   │   ├── main.ts               # Entry: mounts Svelte app
│   │   ├── App.svelte            # Canvas element + mouse handlers + rAF loop
│   │   └── editor.ts             # WASM init + DarklyHandle bridge
│   └── wasm/
│       ├── Cargo.toml            # cdylib crate for wasm-pack
│       ├── .cargo/config.toml    # WASM linker flags (memory, WebGPU)
│       └── src/
│           ├── lib.rs            # WASM entry, panic hook, logging
│           └── api.rs            # DarklyHandle: #[wasm_bindgen] exports
│
└── shaders/
    ├── fullscreen.wgsl           # Fullscreen triangle vertex shader (shared)
    ├── composite.wgsl            # Tile compositing: atlas sample + blend
    ├── blend_modes.wgsl          # Blend mode functions (included by composite)
    ├── present.wgsl              # Final blit to surface
    └── filters/
        └── blur.wgsl             # Gaussian blur filter (Phase 1 demo)
```

---

## Performance Principles

These principles govern how the GPU compositor must behave. They are derived from Krita's raster compositing architecture and Graphite's GPU object management. Violating them will cause catastrophic performance (100% CPU, laggy interface).

### P1: Zero GPU allocation in the render loop

GPU objects (uniform buffers, bind groups, pipelines) are **created once** — at compositor init, when a layer is added, or when the layer stack structure changes. The render loop only:
- Records commands (create encoder, begin render pass, set pipeline, set bind group, draw)
- Uploads dirty tiles via the staging ring (CPU→GPU copy, not allocation)
- Submits the command buffer

**Never** allocate a `wgpu::Buffer`, `wgpu::BindGroup`, or `wgpu::RenderPipeline` inside `render()`. If a uniform value changes (e.g., opacity), update the existing buffer via `queue.write_buffer()`.

Bind groups reference stable objects (accumulator views, layer views, sampler, uniform buffers) that don't change frame-to-frame. They are created when the objects they reference are created, and invalidated only on resize or layer structure changes.

*(Reference: Graphite's `desktop/src/render/state.rs` — render pipeline and bind groups are stored as fields, created once in `new()`, bind groups updated only when texture bindings change via `update_bindgroup()`)*

### P2: No work when nothing changed

The compositor tracks a `needs_composite` flag. If no tiles were uploaded and no layer properties changed since the last render, `render()` skips compositing entirely and re-presents the last frame. The `requestAnimationFrame` loop still runs (for future animation support), but the per-frame cost when idle is effectively zero — just a boolean check and a lightweight present-only blit.

Mutation paths (`paint`, `set_opacity`, `set_blend_mode`, `undo`, `redo`, `add_layer`) set the flag. The render loop clears it after compositing.

*(Reference: Graphite's `surface_outdated` flag in `state.rs:250` — `if !self.surface_outdated { return Ok(()); }`)*

### P3: Cache the composite result — only re-composite from the dirty layer upward

The compositor maintains a **cached composite texture** (GPU-resident, same size as the accumulators). This texture stores the accumulated composite result after compositing all layers. When a layer changes:

1. Find the **lowest dirty layer** in the stack (the layer whose tiles were uploaded or whose properties changed)
2. Copy the cached composite into the accumulator via `encoder.copy_texture_to_texture()` (GPU→GPU, no CPU involvement)
3. Re-composite only from the dirty layer upward, not from scratch
4. Save the final result back to the cached composite texture

**Why this matters:** When the user paints on the top layer (the common case), this skips re-compositing everything below — just one blend pass instead of walking the entire stack. For a 100-layer document where the user paints on the top layer, this reduces compositing work from 100+ render passes to 1.

Cache invalidation:
- Tile upload to layer N → invalidate from layer N upward
- Layer property change (opacity, blend mode) → invalidate from that layer upward
- Layer added/removed/reordered → invalidate entirely
- Canvas resize → recreate the texture

Filter layers propagate naturally — if a layer below a blur changes, the cache is invalidated from that layer, so the blur re-runs. If only a layer above the blur changes, the blur result is preserved in the cache.

*(Reference: Krita's projection system — each layer has a cached `projection()`. Layers below the dirty one are tagged `N_BELOW_FILTHY` and skip recalculation entirely, using the cached result. See `krita-performance.md` §4.2)*

---

## Implementation Steps

### Step 1: Scaffold the project

Create the workspace, crate structure, frontend boilerplate, and build pipeline.

**Workspace `Cargo.toml`:**
- Members: `crates/darkly-core`, `crates/darkly-gpu`, `frontend/wasm`
- Workspace dependencies: `wgpu`, `bytemuck`, `serde`, `wasm-bindgen`, `js-sys`, `web-sys`, `log`

**`darkly-core/Cargo.toml`:** Pure Rust, no WASM deps. Depends on `bytemuck`, `serde`.

**`darkly-gpu/Cargo.toml`:** Depends on `darkly-core`, `wgpu`, `bytemuck`. Feature-gate `web` for WASM-specific surface creation.

**`frontend/wasm/Cargo.toml`:** `crate-type = ["cdylib"]`. Depends on `darkly-core`, `darkly-gpu`, `wasm-bindgen`, `serde-wasm-bindgen`, `js-sys`, `web-sys`, `wgpu`, `console_error_panic_hook`, `console_log`.

**`frontend/wasm/.cargo/config.toml`:**
```toml
[target.wasm32-unknown-unknown]
rustflags = ["--cfg=web_sys_unstable_apis"]

[unstable]
build-std = ["std", "panic_abort"]
```

**`frontend/package.json`:** Svelte 5 + Vite + wasm-pack scripts (modeled on Graphite's `frontend/package.json`).

**`frontend/vite.config.ts`:** Svelte plugin, WASM file serving.

**`frontend/index.html`:** Minimal shell that mounts the Svelte app.

**`frontend/src/main.ts`:** Mount `App.svelte`.

**`frontend/src/App.svelte`:** A full-viewport `<canvas>` element. On mount, calls `editor.ts` to init WASM+GPU. Sets up mouse listeners and rAF loop.

**`frontend/src/editor.ts`:** Loads WASM via `init()`, creates `DarklyHandle`, returns the handle.

**Verification:** `npm run start` builds WASM and serves the page. A blank canvas appears with no errors in console.

---

### Step 2: Tile system (`darkly-core`)

**`tile.rs`:**
```rust
pub const TILE_SIZE: usize = 64;
pub const TILE_BYTES: usize = TILE_SIZE * TILE_SIZE * 4; // RGBA u8

#[derive(Clone)]
pub struct TileData(pub [u8; TILE_BYTES]);  // derive bytemuck::Pod

pub struct Tile {
    pub data: Arc<TileData>,
}

impl Tile {
    pub fn empty() -> Self; // shares a static default Arc
    pub fn write(&mut self) -> &mut TileData; // Arc::make_mut, COW
}

/// Sparse tile grid. Key = (tile_x, tile_y) in tile coordinates.
pub struct TileGrid {
    pub tiles: HashMap<(i32, i32), Tile>,
}

impl TileGrid {
    pub fn new() -> Self;
    pub fn get(&self, tx: i32, ty: i32) -> Option<&Tile>;
    pub fn get_or_create(&mut self, tx: i32, ty: i32) -> &mut Tile;
    pub fn snapshot(&self) -> TileGrid; // Clone — just Arc increments
    pub fn tile_coords_for_pixel(x: u32, y: u32) -> (i32, i32);
}
```

**Verification:** Unit tests — create tiles, write pixels, verify COW cloning behavior.

---

### Step 3: Layers, document, dirty tracking (`darkly-core`)

**`layer.rs`:**
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

pub enum FilterType { GaussianBlur }

pub struct FilterLayer {
    pub id: LayerId,
    pub filter_type: FilterType,
    pub params: FilterParams,  // e.g., blur radius
    pub visible: bool,
}

pub enum Layer {
    Raster(RasterLayer),
    Filter(FilterLayer),
}
```

**`dirty.rs`:**
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

**`document.rs`:**
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
    pub fn add_filter_layer(&mut self, filter_type: FilterType, params: FilterParams) -> LayerId;
    pub fn paint_circle(&mut self, layer_id: LayerId, cx: f32, cy: f32, radius: f32, color: [u8; 4]);
    pub fn fill_gradient(&mut self, layer_id: LayerId); // demo helper
    pub fn layer(&self, id: LayerId) -> Option<&Layer>;
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer>;
}
```

`paint_circle`: iterates tiles touched by the circle's bounding box, calls `tile.write()` on each, sets pixels within radius, marks tiles dirty.

**`undo.rs`:**
```rust
pub struct UndoSnapshot {
    pub tiles: HashMap<LayerId, TileGrid>,  // COW clones
}

pub struct UndoStack {
    snapshots: Vec<UndoSnapshot>,
    cursor: usize,
}

impl UndoStack {
    pub fn push(&mut self, doc: &Document);  // snapshot all layer TileGrids
    pub fn undo(&mut self, doc: &mut Document);
    pub fn redo(&mut self, doc: &mut Document);
}
```

**Verification:** Unit tests — create document, paint, verify dirty tracking, undo/redo.

---

### Step 4: GPU context + surface (`darkly-gpu`)

**`context.rs`:**
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

Init sequence: `Instance::new(Backends::BROWSER_WEBGPU)` → `instance.create_surface_from_canvas()` → `instance.request_adapter()` → `adapter.request_device()` → configure surface.

**Verification:** WASM bridge calls `GpuContext::new()`, surface presents a cleared color each frame.

---

### Step 5: Tile atlas + staging ring (`darkly-gpu`)

**`atlas.rs`:**
```rust
/// GPU-side tile storage. Uses a texture_2d_array where each layer gets slices.
/// Within each slice, tiles are packed in a grid layout.
pub struct TileAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    // Mapping: (layer_index, tile_x, tile_y) → atlas position
    atlas_width_in_tiles: u32,
    atlas_height_in_tiles: u32,
    layer_count: u32,
}

impl TileAtlas {
    pub fn new(device: &wgpu::Device, max_tiles_x: u32, max_tiles_y: u32, max_layers: u32) -> Self;
    pub fn tile_uv(&self, tile_x: u32, tile_y: u32) -> (f32, f32, f32, f32); // UV rect
}
```

For 1920×1080: ceil(1920/64)=30, ceil(1080/64)=17 → 30×17 = 510 tiles per layer. Atlas texture per layer: 1920×1088 (padded to tile boundary).

Simple approach for Phase 1: one `Rgba8Unorm` texture per raster layer (sized to canvas dimensions padded to tile boundary). No complex atlas packing.

**`staging.rs`:**
```rust
pub struct StagingRing {
    buffers: Vec<wgpu::Buffer>,  // MAP_WRITE | COPY_SRC, each TILE_BYTES
    next: usize,
}

impl StagingRing {
    pub fn new(device: &wgpu::Device, count: usize) -> Self;
    pub fn upload_tile(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        tile_data: &TileData,
        target: &wgpu::Texture,
        tile_x: u32,
        tile_y: u32,
    );
}
```

**Verification:** Paint a circle, see the dirty tiles uploaded to the GPU texture (can verify by presenting the raw layer texture).

---

### Step 6: Compositor — blend raster layers (`darkly-gpu`)

**`compositor.rs`:**
```rust
pub struct Compositor {
    /// Two accumulator textures for ping-pong rendering + one blur temp
    accum: [wgpu::Texture; 3],
    accum_views: [wgpu::TextureView; 3],
    current_accum: usize,

    /// Cached composite result (GPU-resident). Stores the final composited
    /// image so we can re-composite from a dirty layer upward instead of
    /// from scratch. See P3.
    composite_cache: wgpu::Texture,
    composite_cache_view: wgpu::TextureView,
    /// Index of the lowest layer that the cache is valid through.
    /// None = cache is empty, must composite from scratch.
    cache_valid_through: Option<usize>,

    /// Per-layer GPU textures (one per raster layer)
    layer_textures: HashMap<LayerId, wgpu::Texture>,
    layer_views: HashMap<LayerId, wgpu::TextureView>,

    /// Pre-built GPU objects per raster layer — created once in
    /// ensure_layer_texture(), never in the render loop (P1).
    raster_cache: HashMap<LayerId, RasterLayerCache>,
    /// Pre-built GPU objects per filter layer — created once when filter
    /// layer is added, never in the render loop (P1).
    filter_cache: HashMap<LayerId, FilterLayerCache>,

    blend_pipelines: BlendPipelines,
    filter_registry: FilterRegistry,
    present_pipeline: wgpu::RenderPipeline,
    present_bind_groups: [wgpu::BindGroup; 2],  // one per possible current_accum

    staging: StagingRing,
    sampler: wgpu::Sampler,

    /// Dirty gate — false means nothing changed, skip compositing (P2)
    needs_composite: bool,

    canvas_width: u32,
    canvas_height: u32,
}

impl Compositor {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat,
               width: u32, height: u32) -> Self;

    /// Create GPU texture + uniform buffer + bind groups for a new layer.
    /// Called once when a layer is added, never in the render loop.
    pub fn ensure_layer_texture(&mut self, device: &wgpu::Device, layer_id: LayerId);

    /// Mark that recompositing is needed. Called by mutation paths
    /// (paint, set_opacity, set_blend_mode, undo, redo, add_layer).
    pub fn mark_dirty(&mut self);

    /// Upload dirty tiles, composite changed layers, present.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
        surface_config: &wgpu::SurfaceConfiguration,
        doc: &mut Document,  // mut to clear dirty flags
    );

    /// Lightweight present when nothing changed — just blit last frame.
    fn present_only(&self, device: &wgpu::Device, queue: &wgpu::Queue,
                    surface: &wgpu::Surface);
}
```

**Render pipeline per frame:**
1. Upload dirty tiles for each dirty raster layer via staging ring → layer texture. If any tiles were uploaded, note the lowest dirty layer index and set `needs_composite = true`.
2. **Dirty gate (P2):** if `!needs_composite`, call `present_only()` and return.
3. **Composite cache (P3):** if `cache_valid_through` is set and >= 0, copy the cached composite into accumulator[0] via `copy_texture_to_texture()` and start compositing from `cache_valid_through + 1` instead of layer 0.
4. For each layer from the start point to the top, bottom-to-top:
   - **Raster:** set pre-built bind group for the current ping-pong direction, set pipeline, draw. Ping-pong.
   - **Filter:** look up pipeline from filter registry, set pre-built bind groups from filter cache, draw each pass. Ping-pong.
5. Save the final accumulator to `composite_cache` via `copy_texture_to_texture()`. Update `cache_valid_through`.
6. Present: blit accumulator to surface using pre-built present bind group.
7. Clear dirty regions, set `needs_composite = false`.

**`blend.rs`:**
```rust
pub struct BlendPipelines {
    pipeline: wgpu::RenderPipeline,  // single pipeline, blend mode is a uniform
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl BlendPipelines {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self;
    pub fn get(&self, mode: BlendMode) -> &wgpu::RenderPipeline;
}
```

The blend pipeline uses a fullscreen triangle vertex shader and a fragment shader that samples the background accumulator and the layer texture, applies the blend equation (selected by uniform), and outputs the result. The bind group layout defines bindings for: accumulator texture, layer texture, sampler, and uniforms buffer. Per-layer bind groups and uniform buffers are created once in `Compositor::ensure_layer_texture()` (P1), not per-frame.

**Shaders:**

`fullscreen.wgsl` — shared vertex shader, no vertex buffer:
```wgsl
@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}
```

`composite.wgsl` — compositing fragment shader:
```wgsl
@group(0) @binding(0) var t_bg: texture_2d<f32>;      // accumulator
@group(0) @binding(1) var t_layer: texture_2d<f32>;    // current layer
@group(0) @binding(2) var t_sampler: sampler;

struct Uniforms { opacity: f32, blend_mode: u32 }
@group(0) @binding(3) var<uniform> uniforms: Uniforms;

@fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = pos.xy / vec2f(textureDimensions(t_bg));
    let bg = textureSample(t_bg, t_sampler, uv);
    var fg = textureSample(t_layer, t_sampler, uv);
    fg.a *= uniforms.opacity;
    return blend(fg, bg, uniforms.blend_mode);
}
```

`blend_modes.wgsl` — blend function:
```wgsl
fn blend(fg: vec4f, bg: vec4f, mode: u32) -> vec4f {
    let fg_pre = fg.rgb * fg.a;
    let bg_pre = bg.rgb * bg.a;
    var out_rgb: vec3f;
    switch mode {
        case 0, and user-u: { out_rgb = fg_pre; } // Normal
        case 1u: { out_rgb = fg_pre * bg_pre; } // Multiply (simplified)
        case 2u: { out_rgb = fg_pre + bg_pre - fg_pre * bg_pre; } // Screen
        default: { out_rgb = fg_pre; }
    }
    let out_a = fg.a + bg.a * (1.0 - fg.a);
    return vec4f(mix(bg_pre, out_rgb, fg.a) / max(out_a, 0.001), out_a);
}
```

**Verification:** Create 2 raster layers, fill bottom with gradient, paint on top. Both layers composite correctly with Normal blend mode.

---

### Step 7: Filter shader system (`darkly-gpu`)

The filter system is a **modular registry** of GPU filter pipelines. Each filter type (blur, noise, sharpen, etc.) registers its pipeline and bind group layout once at init. Per-filter-layer instance state (uniform buffers, bind groups) is cached in the compositor alongside raster layer caches (P1). Adding a new filter means writing a shader, implementing the `Filter` trait, and registering it — no changes to the compositor or the render loop.

Phase 1 ships one filter (Gaussian blur) to prove the system works end-to-end. The system itself is not blur-specific.

**`layer.rs` — filter params as an enum:**
```rust
#[derive(Clone, Debug)]
pub enum FilterParams {
    GaussianBlur { radius: f32 },
    // Future: Noise { seed: u32, intensity: f32 }, Sharpen { amount: f32 }, etc.
}
```

Each variant carries only the parameters specific to that filter type. The `FilterType` enum mirrors it for type-level dispatch without the parameter values:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilterType {
    GaussianBlur,
    // Future: Noise, Sharpen, etc.
}

impl FilterParams {
    pub fn filter_type(&self) -> FilterType {
        match self {
            FilterParams::GaussianBlur { .. } => FilterType::GaussianBlur,
        }
    }
}
```

**`filter.rs` — filter registry:**
```rust
/// Describes a single filter type's GPU resources (pipeline + layout).
/// Created once at registration, never in the render loop.
struct FilterEntry {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// Number of render passes this filter requires (e.g., blur = 2 for H+V).
    pass_count: u32,
    /// Size in bytes of this filter's uniform struct.
    uniform_size: u32,
}

/// Registry of all available filter pipelines.
/// Pure infrastructure — holds pipelines and layouts, no per-instance state.
pub struct FilterRegistry {
    filters: HashMap<FilterType, FilterEntry>,
}

impl FilterRegistry {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let mut registry = FilterRegistry { filters: HashMap::new() };
        registry.register_blur(device, format);
        // Future: registry.register_noise(device, format);
        registry
    }

    pub fn entry(&self, filter_type: FilterType) -> &FilterEntry;
    pub fn pipeline(&self, filter_type: FilterType) -> &wgpu::RenderPipeline;
    pub fn bind_group_layout(&self, filter_type: FilterType) -> &wgpu::BindGroupLayout;

    fn register_blur(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat);
}
```

**Per-filter-layer GPU cache (in compositor):**

The compositor manages per-filter-layer cached GPU objects the same way it manages per-raster-layer caches. When a filter layer is added, the compositor creates uniform buffers and bind groups for it using the registry's bind group layout:

```rust
/// Cached GPU objects for a filter layer instance (P1).
struct FilterLayerCache {
    /// One uniform buffer per pass (e.g., blur has 2: H and V directions).
    uniform_bufs: Vec<wgpu::Buffer>,
    /// One bind group per pass, per ping-pong direction.
    /// Indexed as bind_groups[ping_pong_src][pass_index].
    bind_groups: Vec<[wgpu::BindGroup; 2]>,
}
```

This lives in the compositor's cache alongside raster caches, keyed by `LayerId`. The filter registry provides the pipeline and layout; the compositor owns the instance state.

**Compositor render loop — generic filter dispatch:**

```rust
Layer::Filter(filter) => {
    let entry = filter_registry.entry(filter.filter_type);
    let cache = &filter_layer_cache[&filter.id];
    for pass in 0..entry.pass_count {
        let mut rpass = encoder.begin_render_pass(...);
        rpass.set_pipeline(&entry.pipeline);
        rpass.set_bind_group(0, &cache.bind_groups[pass][current_accum], &[]);
        rpass.draw(0..3, 0..1);
        // ping-pong between accumulators
    }
}
```

No `match filter.filter_type` in the render loop — the registry and cached bind groups handle dispatch generically. The compositor doesn't know or care what kind of filter it's running.

**Blur shader (`filters/blur.wgsl`):**

```wgsl
struct BlurParams { radius: f32, direction: vec2f, _pad: f32 }
@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

@fragment fn fs_blur(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(t_input));
    let uv = pos.xy / dims;
    let step = params.direction / dims;
    var color = vec4f(0.0);
    var weight_sum = 0.0;
    let r = i32(params.radius);
    for (var i = -r; i <= r; i++) {
        let w = 1.0 - abs(f32(i)) / (params.radius + 1.0); // triangle kernel
        color += textureSample(t_input, t_sampler, uv + step * f32(i)) * w;
        weight_sum += w;
    }
    return color / weight_sum;
}
```

Blur is registered with `pass_count = 2` (H then V) and consumes two ping-pong flips.

**Verification:** Add blur filter between two raster layers. Bottom layer has a gradient, top layer is painted. The blur obscures the composite-so-far when viewed from above.

---

### Step 8: WASM bridge + frontend integration

**`frontend/wasm/src/lib.rs`:**
- `#[wasm_bindgen(start)]` sets panic hook + console logger
- Thread-local state for the `DarklyHandle`

**`frontend/wasm/src/api.rs`:**
```rust
#[wasm_bindgen]
pub struct DarklyHandle {
    doc: Document,
    compositor: Compositor,
    gpu: GpuContext,
    undo_stack: UndoStack,
}

#[wasm_bindgen]
impl DarklyHandle {
    pub async fn create(canvas: web_sys::HtmlCanvasElement) -> DarklyHandle;
    pub fn paint(&mut self, layer_id: u64, x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8);
    pub fn render(&mut self);
    pub fn add_raster_layer(&mut self) -> u64;
    pub fn add_filter_layer(&mut self, filter_type: u32, param: f32) -> u64;
    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32);
    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32);
    pub fn undo(&mut self);
    pub fn redo(&mut self);
    pub fn snapshot(&mut self); // push undo snapshot
}
```

**`frontend/src/App.svelte`:**
- On mount: call `initEditor(canvas)` from `editor.ts`
- Mouse move with button down → `handle.paint(activeLayerId, x, y, brushRadius, ...color)`
- `requestAnimationFrame` loop → `handle.render()` (compositor handles dirty gate internally per P2 — idle frames are near-free)
- On mount after init: hardcode demo setup:
  ```ts
  const bg = handle.add_raster_layer();
  handle.fill_gradient(bg);             // pre-fill
  const blur = handle.add_filter_layer(0, 8.0); // blur, radius=8
  const paint = handle.add_raster_layer();
  // mouse painting targets `paint` layer
  ```

**Verification:** Full end-to-end: open browser, see gradient background, paint with mouse, blur filter softens everything.

---

## Build Order Summary

| Step | Deliverable | Test |
|------|-------------|------|
| 1 | Project scaffold, WASM builds, blank canvas appears | `npm run start` shows canvas |
| 2 | Tile system with COW | `cargo test` — unit tests pass |
| 3 | Layers, document, dirty tracking, undo | `cargo test` — paint + undo tests |
| 4 | GPU context + surface | Colored rectangle clears to screen |
| 5 | Tile upload to GPU | Painted tiles visible on screen |
| 6 | Compositor with blend modes | Two layers composite correctly |
| 7 | Blur filter shader | Filter obscures composite |
| 8 | Full WASM bridge + demo | Paint behind the blur, end-to-end |

## Key Reference Files (Graphite, patterns only)

- WASM entry: `Graphite/frontend/wasm/src/lib.rs`
- WASM API: `Graphite/frontend/wasm/src/editor_api.rs`
- GPU context: `Graphite/node-graph/libraries/wgpu-executor/src/context.rs`
- Shader runtime: `Graphite/node-graph/libraries/wgpu-executor/src/shader_runtime/`
- Frontend init: `Graphite/frontend/src/editor.ts`
- Package setup: `Graphite/frontend/package.json`
- Composite shader: `Graphite/desktop/src/render/composite_shader.wgsl`
