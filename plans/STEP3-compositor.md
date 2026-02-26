# Phase 1, Session 3 — Tile Upload + Compositor

## Scope

Steps 5–6 from the Phase 1 plan. Implement the staging ring for CPU->GPU tile upload, then build the full compositor with ping-pong rendering, blend modes, composite caching, and scissor-rect optimization.

## Prerequisites

Session 2 complete: layer types, document model, dirty tracking, and undo stack implemented. GPU context initializes and clears a color to the surface. All unit tests pass.

## Done When

- Two raster layers composite correctly on screen (bottom gradient + top painted layer with Normal blend mode)
- Dirty tracking and composite caching work (idle frames skip GPU work)

---

## Performance Principles (must be followed)

### P1: Zero GPU allocation in the render loop

GPU objects (uniform buffers, bind groups, pipelines) are **created once** — at compositor init, when a layer is added, or when the layer stack structure changes. The render loop only:
- Records commands (create encoder, begin render pass, set pipeline, set bind group, draw)
- Uploads dirty tiles via the staging ring (CPU->GPU copy, not allocation)
- Submits the command buffer

**Never** allocate a `wgpu::Buffer`, `wgpu::BindGroup`, or `wgpu::RenderPipeline` inside `render()`.

### P2: No work when nothing changed

The compositor tracks a `needs_composite` flag. If no tiles were uploaded and no layer properties changed since the last render, `render()` returns immediately — no surface acquisition, no command encoder, no GPU submission. Zero GPU interaction.

**Never** call `surface.get_current_texture()`, `device.create_command_encoder()`, or `queue.submit()` when nothing has changed.

### P3: Only composite dirty layers within the dirty rect

- **Vertically (layers):** Skip all layers below the lowest dirty layer using the cached composite texture.
- **Horizontally (pixels):** Clip all blend/filter passes to the bounding rect of dirty tiles via `rpass.set_scissor_rect()`.

**Composite cache:** The compositor maintains a cached composite texture (GPU-resident). When a layer changes:
1. Find the lowest dirty layer in the stack
2. Compute dirty bounding rect from `DirtyRegion::bounding_rect()`, expanded to tile boundaries
3. Re-composite only from the dirty layer upward, only within the dirty rect
4. Copy only the dirty rect from the accumulator back to the cached composite texture

**Cache invalidation:** Any dirty layer means full invalidation (`cache_valid_through = None`).

---

## Step 5: Tile atlas + staging ring

### `gpu/atlas.rs`

Simple approach for Phase 1: one `Rgba8Unorm` texture per raster layer (sized to canvas dimensions padded to tile boundary). No complex atlas packing. For 1920x1080: ceil(1920/64)=30, ceil(1080/64)=17. Layer texture: 1920x1088.

```rust
pub struct TileAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    atlas_width_in_tiles: u32,
    atlas_height_in_tiles: u32,
    layer_count: u32,
}

impl TileAtlas {
    pub fn new(device: &wgpu::Device, max_tiles_x: u32, max_tiles_y: u32, max_layers: u32) -> Self;
    pub fn tile_uv(&self, tile_x: u32, tile_y: u32) -> (f32, f32, f32, f32);
}
```

### `gpu/staging.rs`

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

### Verification

Paint a circle, see the dirty tiles uploaded to the GPU texture (can verify by presenting the raw layer texture).

---

## Step 6: Compositor — blend raster layers

### `gpu/compositor.rs`

```rust
pub struct Compositor {
    /// Two accumulator textures for ping-pong rendering
    accum: [wgpu::Texture; 2],
    accum_views: [wgpu::TextureView; 2],
    current_accum: usize,

    /// Cached composite result (GPU-resident)
    composite_cache: wgpu::Texture,
    composite_cache_view: wgpu::TextureView,
    cache_valid_through: Option<usize>,

    /// Per-layer GPU textures (one per raster layer)
    layer_textures: HashMap<LayerId, wgpu::Texture>,
    layer_views: HashMap<LayerId, wgpu::TextureView>,

    /// Pre-built GPU objects per raster layer (P1)
    raster_cache: HashMap<LayerId, RasterLayerCache>,
    /// Pre-built GPU objects per filter layer (P1)
    filter_cache: HashMap<LayerId, FilterLayerCache>,

    blend_pipelines: BlendPipelines,
    filter_registry: FilterRegistry,
    present_pipeline: wgpu::RenderPipeline,
    present_bind_groups: [wgpu::BindGroup; 2],

    staging: StagingRing,
    sampler: wgpu::Sampler,

    needs_composite: bool,
    canvas_width: u32,
    canvas_height: u32,
}
```

### Render pipeline per frame

1. **Upload dirty tiles** for each dirty raster layer via staging ring -> layer texture. If any tiles were uploaded, note the lowest dirty layer index and set `needs_composite = true`.
   - **Pitfall — removed tiles:** Undo can remove tiles from the grid (tile didn't exist before the stroke). When a dirty tile coordinate has no tile in the grid, upload a static blank (transparent) `TileData` to clear stale GPU data.
   - **Pitfall — out-of-bounds tiles:** Painting near canvas edges creates tiles at coordinates beyond the GPU texture dimensions. Skip tiles where `tx >= layer_tex.width_in_tiles` or `ty >= layer_tex.height_in_tiles`.
2. **Dirty gate (P2):** if `!needs_composite`, return immediately.
3. **Compute dirty bounding rect (P3):** Union all `DirtyRegion::bounding_rect()` across dirty layers into a single pixel-coordinate rect, expanded to tile boundaries.
4. **Composite cache (P3):** if `cache_valid_through` is set, start compositing from there + 1 instead of layer 0.
5. **For each layer** from the start point to the top:
   - Set `rpass.set_scissor_rect(dirty_rect)` on every render pass. Use `LoadOp::Load`.
   - **Raster:** set pre-built bind group, set pipeline, draw. Ping-pong.
   - **Filter:** (deferred to Session 4 — filter dispatch skeleton can be here but noise filter not yet registered)
6. **Copy dirty rect** from final accumulator to `composite_cache`.
7. **Present:** blit `composite_cache` to surface. Present shader derives UVs from `position.xy / textureDimensions(t_source)`.
8. Clear dirty regions, set `needs_composite = false`.

### `gpu/blend.rs`

```rust
pub struct BlendPipelines {
    pipeline: wgpu::RenderPipeline,  // single pipeline, blend mode is a uniform
    pub bind_group_layout: wgpu::BindGroupLayout,
}
```

The blend pipeline uses a fullscreen triangle vertex shader and a fragment shader that samples the background accumulator and the layer texture, applies the blend equation (selected by uniform), and outputs the result.

### Shaders

**`shaders/fullscreen.wgsl`** — shared vertex shader, no vertex buffer:
```wgsl
@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}
```

**`shaders/composite.wgsl`** — compositing fragment shader:
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

**`shaders/blend_modes.wgsl`** — blend function:
```wgsl
fn blend(fg: vec4f, bg: vec4f, mode: u32) -> vec4f {
    let fg_pre = fg.rgb * fg.a;
    let bg_pre = bg.rgb * bg.a;
    var out_rgb: vec3f;
    switch mode {
        case 0u: { out_rgb = fg_pre; } // Normal
        case 1u: { out_rgb = fg_pre * bg_pre; } // Multiply (simplified)
        case 2u: { out_rgb = fg_pre + bg_pre - fg_pre * bg_pre; } // Screen
        default: { out_rgb = fg_pre; }
    }
    let out_a = fg.a + bg.a * (1.0 - fg.a);
    return vec4f(mix(bg_pre, out_rgb, fg.a) / max(out_a, 0.001), out_a);
}
```

**`shaders/present.wgsl`** — final blit to surface.

### Verification

Create 2 raster layers, fill bottom with gradient, paint on top. Both layers composite correctly with Normal blend mode. Verify idle frames skip GPU work (P2).

---

## Key Reference Files (Graphite, patterns only)

- GPU context: `Graphite/node-graph/libraries/wgpu-executor/src/context.rs`
- Shader runtime: `Graphite/node-graph/libraries/wgpu-executor/src/shader_runtime/`
- Composite shader: `Graphite/desktop/src/render/composite_shader.wgsl`
- State management (P1/P2 patterns): `Graphite/desktop/src/render/state.rs`
