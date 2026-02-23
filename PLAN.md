# Darkly Phase 1 — Layer System + GPU Compositor

## Context

Darkly is a browser-based art tool that uses "Veils" (filter layers) to obscure and transform artwork, stimulating creative exploration. Phase 1 establishes the foundation: a tiled raster layer system with GPU compositing and filter shaders. No UI — just a canvas with hardcoded layers and mouse painting.

The project is a new standalone Rust+WASM+Svelte codebase at `/mega/ARTEXP/darkly/`. The Graphite editor at `/mega/ARTEXP/darkly/Graphite/` is reference only. Krita also is at `/mega/ARTEXP/darkly/krita/` if needed.

**Demo goal:** Two raster layers + a noise filter layer between them. Mouse paints on the top layer. The noise filter obscures the painting with a procedural grain overlay. The bottom layer has a pre-filled color gradient. This proves tiles, dirty tracking, GPU compositing, blend modes, and filter shaders all work end-to-end.

**Engineering principle:** The core engine does not need to be 100% implemented, but every part that is implemented must be implemented properly on the first iteration. No hacks, no hardcoding, no shortcuts in the engine. If we implement one filter, we build a proper modular filter system and register that one filter in it. If we implement one blend mode, we build a proper blend mode system. The frontend (TypeScript/Svelte) can hardcode and cut corners freely — Rust code cannot, including the WASM bridge. This applies to every system: tiles, layers, filters, compositing, undo, GPU resource management.

**Modularity principle:** Generic systems must not contain domain-specific knowledge. The layer system must not know what filter types exist. The compositor render loop must not branch on filter type. Each filter is a self-contained module that implements a trait and registers itself — adding a new filter means creating a new file, not modifying existing ones. If an enum would need a new variant every time a module is added, use a trait object instead.

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
│           ├── filter.rs         # FilterHandler trait, FilterRegistry, FilterLayerCache
│           └── filters/
│               └── noise.rs      # NoiseParams, NoiseHandler (self-contained filter module)
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
        └── noise.wgsl            # Noise overlay filter (Phase 1 demo)
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

The compositor tracks a `needs_composite` flag. If no tiles were uploaded and no layer properties changed since the last render, `render()` returns immediately — no surface acquisition, no command encoder, no GPU submission, no present. Zero GPU interaction. The browser compositor retains and continues displaying the last presented surface frame; there is no need to re-blit it. The `requestAnimationFrame` loop still runs (for future animation support), but the per-frame cost when idle is effectively zero — just a boolean check and an early return.

**Never** call `surface.get_current_texture()`, `device.create_command_encoder()`, or `queue.submit()` when nothing has changed. Each of these triggers real GPU/compositor work even if the visual output is identical. The idle path must touch no GPU APIs whatsoever.

Mutation paths (`paint`, `set_opacity`, `set_blend_mode`, `undo`, `redo`, `add_layer`) set the flag. The render loop clears it after compositing.

*(Reference: Graphite's `surface_outdated` flag in `state.rs:250` — `if !self.surface_outdated { return Ok(()); }`)*

### P3: Only composite dirty layers within the dirty rect

Compositing work is bounded in two dimensions:

- **Vertically (layers):** Skip all layers below the lowest dirty layer using the cached composite texture.
- **Horizontally (pixels):** Clip all blend/filter passes to the bounding rect of dirty tiles via `rpass.set_scissor_rect()`. Only pixels within the dirty rect invoke the fragment shader.

Both are required. Skipping layers without scissoring still processes every pixel on the canvas per pass. Scissoring without layer caching still walks the entire layer stack. Together they reduce compositing work from `O(layers × canvas_pixels)` to `O(dirty_layers × dirty_pixels)`.

**Composite cache:** The compositor maintains a **cached composite texture** (GPU-resident, same size as the accumulators). This texture stores the accumulated composite result after compositing all layers. When a layer changes:

1. Find the **lowest dirty layer** in the stack (the layer whose tiles were uploaded or whose properties changed)
2. Compute the **dirty bounding rect** in pixel coordinates from `DirtyRegion::bounding_rect()`, expanded to tile boundaries
3. Re-composite only from the dirty layer upward, only within the dirty rect
4. Copy only the dirty rect from the accumulator back to the cached composite texture

**Scissor-rect compositing:** Every blend and filter render pass calls `rpass.set_scissor_rect(x, y, w, h)` with the dirty bounding rect. The fullscreen triangle vertex shader still runs, but the rasterizer clips to the scissor — only fragments inside invoke the fragment shader. Render passes use `LoadOp::Load` (not `LoadOp::Clear`) to preserve pixels outside the scissor from the previous frame. The `copy_texture_to_texture` between accumulator and cache is also scoped to the dirty rect via the `origin` and `size` parameters.

**Why this matters:** When the user paints a 24px brush dab on the top layer of a 1920×1080 canvas, the dirty rect is ~128×64 pixels (2 tiles). Instead of processing 2,073,600 pixels across every layer, the compositor processes ~8,192 pixels across 1 layer. On software wgpu (no hardware GPU), this is the difference between ~56 MB/frame and ~32 KB/frame of CPU pixel processing — a ~1700× reduction.

**Cache invalidation:**
- Tile upload to layer N → invalidate from layer N upward
- Layer property change (opacity, blend mode) → invalidate from that layer upward
- Layer added/removed/reordered → invalidate entirely
- Canvas resize → recreate the texture

Filter layers propagate naturally — if a layer below a filter changes, the cache is invalidated from that layer, so the filter re-runs. If only a layer above the filter changes, the filter result is preserved in the cache.

*(Reference: Krita's projection system — each layer has a cached `projection()`. Layers below the dirty one are tagged `N_BELOW_FILTHY` and skip recalculation entirely, using the cached result. Krita composites only dirty rects tile-by-tile on CPU; the GPU equivalent is scissor-rect clipping.)*

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

/// String identifier for a filter type (e.g., "noise", "blur").
/// Each filter module defines its own constant. The layer system
/// never interprets this — it's an opaque key for the filter registry.
pub type FilterTypeId = &'static str;

/// Trait for filter parameters. Implemented by each filter module.
/// The layer system only sees this trait — never concrete param types.
pub trait FilterParams: std::fmt::Debug + Send + Sync {
    fn filter_type_id(&self) -> FilterTypeId;
    fn clone_boxed(&self) -> Box<dyn FilterParams>;
    fn as_any(&self) -> &dyn std::any::Any;
}

pub struct FilterLayer {
    pub id: LayerId,
    pub params: Box<dyn FilterParams>,
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
    pub fn add_filter_layer(&mut self, params: Box<dyn FilterParams>) -> LayerId;
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

Simple approach for Phase 1: one `Rgba8Unorm` texture per raster layer (sized to canvas dimensions padded to tile boundary). No complex atlas packing. Accumulator and composite cache textures are padded the same way so all textures sampled with the same UVs share identical dimensions.

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
    /// Two accumulator textures for ping-pong rendering
    accum: [wgpu::Texture; 2],
    accum_views: [wgpu::TextureView; 2],
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
2. **Dirty gate (P2):** if `!needs_composite`, return immediately (no surface acquisition, no GPU work).
3. **Compute dirty bounding rect (P3):** Union all `DirtyRegion::bounding_rect()` across dirty layers into a single pixel-coordinate rect, expanded to tile boundaries. This is the scissor rect for all compositing passes.
4. **Composite cache (P3):** if `cache_valid_through` is set and >= 0, start compositing from `cache_valid_through + 1` instead of layer 0. The first blend pass reads from the cached composite texture (via a pre-built bind group) instead of a cleared accumulator.
5. For each layer from the start point to the top, bottom-to-top:
   - Set `rpass.set_scissor_rect(dirty_rect)` on every render pass. Use `LoadOp::Load` to preserve pixels outside the scissor.
   - **Raster:** set pre-built bind group for the current ping-pong direction, set pipeline, draw. Ping-pong.
   - **Filter:** look up pipeline from filter registry, set pre-built bind groups from filter cache, draw each pass. Ping-pong.
6. Copy only the dirty rect from the final accumulator to `composite_cache` via `copy_texture_to_texture()` with scoped `origin` and `size`. Update `cache_valid_through`.
7. Present: blit `composite_cache` to surface. The present fragment shader derives UVs from `position.xy / textureDimensions(t_source)` since the cache is padded larger than the surface.
8. Clear dirty regions, set `needs_composite = false`.

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

The filter system is a **modular registry** of GPU filter pipelines. Each filter type (noise, blur, sharpen, etc.) registers its pipeline and bind group layout once at init. Per-filter-layer instance state (uniform buffers, bind groups, textures) is cached in the compositor alongside raster layer caches (P1). Adding a new filter means writing a shader, implementing the `Filter` trait, and registering it — no changes to the compositor or the render loop.

Phase 1 ships one filter (noise overlay) to prove the system works end-to-end. The system itself is not noise-specific.

**Noise params (defined in the noise filter module, NOT in `layer.rs`):**
```rust
// e.g., in darkly-gpu/src/filters/noise.rs
pub const FILTER_TYPE: FilterTypeId = "noise";

#[derive(Clone, Debug)]
pub struct NoiseParams {
    /// Strength of the noise effect (0.0–1.0).
    pub amount: f32,
    /// Size in pixels of each noise "cell". 1 = per-pixel noise,
    /// 4 = each 4×4 block shares the same noise value (coarser grain).
    pub resolution: u32,
}

impl FilterParams for NoiseParams {
    fn filter_type_id(&self) -> FilterTypeId { FILTER_TYPE }
    fn clone_boxed(&self) -> Box<dyn FilterParams> { Box::new(self.clone()) }
    fn as_any(&self) -> &dyn Any { self }
}
```

The `FilterParams` trait is defined in `layer.rs` (see Step 3). `layer.rs` has zero knowledge of `NoiseParams` or any other concrete filter type.

**`filter.rs` — filter registry:**
```rust
/// Per-filter-type GPU resources + a factory for creating per-instance state.
/// Implemented by each filter module, registered once at init.
pub trait FilterHandler: Send + Sync {
    /// Number of render passes this filter requires (noise = 1).
    fn pass_count(&self) -> u32;
    /// The pipeline for this filter's shader.
    fn pipeline(&self) -> &wgpu::RenderPipeline;
    /// The bind group layout for this filter's shader.
    fn bind_group_layout(&self) -> &wgpu::BindGroupLayout;
    /// Deserialize filter params from a JS object. Called by the WASM bridge
    /// so it doesn't need to know concrete param types.
    fn create_params(&self, js: wasm_bindgen::JsValue) -> Box<dyn FilterParams>;
    /// Create per-instance GPU state (uniform buffers, bind groups, aux textures)
    /// for a newly added filter layer. Called once at layer creation (P1).
    fn create_instance(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &dyn FilterParams,
        accum_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        canvas_width: u32,
        canvas_height: u32,
    ) -> FilterLayerCache;
}

/// Registry of all available filter pipelines.
/// Pure infrastructure — maps FilterTypeId to handlers, no per-instance state.
pub struct FilterRegistry {
    handlers: HashMap<FilterTypeId, Box<dyn FilterHandler>>,
}

impl FilterRegistry {
    pub fn new() -> Self {
        FilterRegistry { handlers: HashMap::new() }
    }
    pub fn register(&mut self, id: FilterTypeId, handler: Box<dyn FilterHandler>);
    pub fn get(&self, id: FilterTypeId) -> Option<&dyn FilterHandler>;
}
```

Registration happens at compositor init — each filter module provides a function that creates its handler:
```rust
// In Compositor::new():
let mut filter_registry = FilterRegistry::new();
filter_registry.register(
    noise::FILTER_TYPE,
    Box::new(noise::NoiseHandler::new(device, format)),
);
// Future: filter_registry.register(blur::FILTER_TYPE, ...);
```

The registry itself has no filter-specific code. Adding a new filter means writing a new module that implements `FilterHandler` and a single `register()` call.

**Per-filter-layer GPU cache (in compositor):**

The compositor manages per-filter-layer cached GPU objects the same way it manages per-raster-layer caches. When a filter layer is added, the compositor calls `handler.create_instance()` which returns a `FilterLayerCache`:

```rust
/// Cached GPU objects for a filter layer instance (P1).
struct FilterLayerCache {
    /// One uniform buffer per pass.
    uniform_bufs: Vec<wgpu::Buffer>,
    /// One bind group per pass, per ping-pong direction.
    /// Indexed as bind_groups[pass_index][ping_pong_src].
    bind_groups: Vec<[wgpu::BindGroup; 2]>,
    /// Optional auxiliary textures (e.g., noise texture for noise filter).
    aux_textures: Vec<wgpu::Texture>,
    aux_views: Vec<wgpu::TextureView>,
}
```

This lives in the compositor's cache alongside raster caches, keyed by `LayerId`. The filter registry provides the handler; the handler's `create_instance()` builds instance state. The compositor stores it but never inspects it.

**Noise texture generation (inside `NoiseHandler::create_instance`):**

When a noise filter layer is created, the handler downcasts `&dyn FilterParams` to `&NoiseParams` via `as_any()`, then:

1. Computes the noise texture dimensions: `ceil(canvas_width / resolution) × ceil(canvas_height / resolution)`.
2. Fills with random `u8` values (one channel — luminance noise) using a simple PRNG seeded from the layer ID.
3. Uploads to a `R8Unorm` GPU texture via `queue.write_texture()`.
4. Stores the texture in the returned `FilterLayerCache::aux_textures`.

The noise texture is static — generated once at layer creation. It does not change per-frame. The shader samples this texture and uses it to modulate the input image. All of this logic lives in the noise module — the compositor just calls `handler.create_instance()` generically.

**Compositor render loop — generic filter dispatch:**

```rust
Layer::Filter(filter) => {
    let type_id = filter.params.filter_type_id();
    let handler = filter_registry.get(type_id).unwrap();
    let cache = &filter_layer_cache[&filter.id];
    for pass in 0..handler.pass_count() {
        let mut rpass = encoder.begin_render_pass(...);
        rpass.set_pipeline(handler.pipeline());
        rpass.set_bind_group(0, &cache.bind_groups[pass][current_accum], &[]);
        rpass.draw(0..3, 0..1);
        // ping-pong between accumulators
    }
}
```

No `match` on filter type in the render loop — the registry trait objects and cached bind groups handle dispatch generically. The compositor doesn't know or care what kind of filter it's running.

**Noise shader (`filters/noise.wgsl`):**

```wgsl
struct NoiseParams { amount: f32, resolution: f32, _pad0: f32, _pad1: f32 }
@group(0) @binding(0) var t_input: texture_2d<f32>;    // accumulator (composite so far)
@group(0) @binding(1) var t_noise: texture_2d<f32>;    // pre-generated noise texture (R8)
@group(0) @binding(2) var t_sampler: sampler;
@group(0) @binding(3) var<uniform> params: NoiseParams;

@fragment fn fs_noise(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(t_input));
    let uv = pos.xy / dims;
    let color = textureSample(t_input, t_sampler, uv);

    // Sample noise texture — UV maps pixel position to noise cell
    let noise_val = textureSample(t_noise, t_sampler, uv).r;

    // Overlay blend: brightens highlights, darkens shadows
    let noise_rgb = vec3f(noise_val);
    var blended: vec3f;
    // Overlay blend mode: 2*a*b if a < 0.5, else 1 - 2*(1-a)*(1-b)
    let base = color.rgb;
    let lo = 2.0 * base * noise_rgb;
    let hi = 1.0 - 2.0 * (1.0 - base) * (1.0 - noise_rgb);
    blended = select(hi, lo, base < vec3f(0.5));

    // Mix between original and blended by amount
    return vec4f(mix(color.rgb, blended, params.amount), color.a);
}
```

Noise is registered with `pass_count = 1` — a single overlay pass. The noise texture binding is included in the bind group created at filter layer init, so no additional setup is needed in the render loop.

**Verification:** Add noise filter between two raster layers. Bottom layer has a gradient, top layer is painted. The noise filter applies a visible grain overlay to the composite-so-far when viewed from above. Adjusting `amount` changes intensity, adjusting `resolution` changes grain size.

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
    /// Accepts a filter type string and a JsValue object of params.
    /// Delegates to the filter registry to deserialize params generically.
    pub fn add_filter_layer(&mut self, filter_type: &str, params: JsValue) -> u64;
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
  const noise = handle.add_filter_layer("noise", { amount: 0.5, resolution: 2 });
  const paint = handle.add_raster_layer();
  // mouse painting targets `paint` layer
  ```

**Verification:** Full end-to-end: open browser, see gradient background, paint with mouse, noise filter applies grain overlay to the composite.

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
| 7 | Noise filter shader | Filter applies grain to composite |
| 8 | Full WASM bridge + demo | Paint with noise overlay, end-to-end |

## Key Reference Files (Graphite, patterns only)

- WASM entry: `Graphite/frontend/wasm/src/lib.rs`
- WASM API: `Graphite/frontend/wasm/src/editor_api.rs`
- GPU context: `Graphite/node-graph/libraries/wgpu-executor/src/context.rs`
- Shader runtime: `Graphite/node-graph/libraries/wgpu-executor/src/shader_runtime/`
- Frontend init: `Graphite/frontend/src/editor.ts`
- Package setup: `Graphite/frontend/package.json`
- Composite shader: `Graphite/desktop/src/render/composite_shader.wgsl`
