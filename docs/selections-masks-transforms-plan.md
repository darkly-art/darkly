# Selections, Masks, Copy/Paste, Transforms & Tool Overlay

## Context

Darkly is a GPU-accelerated photo editor running in the browser via WASM. The core engine (Rust) has a tile-based compositor with COW undo, blend modes, filters, and veils. We need to add the remaining core editing features: selections, layer masks, copy/paste, and transforms. Analysis of Krita and Graphite showed that the key insight is a unified `AlphaMask` primitive (single-channel tile grid) that selections, layer masks, and future systems all share.

The user also requested a **generic GPU tool overlay system** — any tool can annotate the canvas with lines, handles, and shapes rendered in inverted color for maximum visibility. This replaces the current SVG-based `ToolOverlay.svelte` for visual rendering and also powers marching ants.

---

## Phase 1: Root Group Refactor + Generic Tile Storage

**Goal:** (a) Make the document root a `LayerGroup` so all layer tree logic is a single recursive path. (b) Make `TileGrid` generic so both RGBA layers and single-channel masks share the same COW/transaction/memento infrastructure.

**Why first:** The root group refactor eliminates the "root vec vs group children" dualism that would otherwise force special cases in compositing, mask propagation, and flatten. Everything else depends on `AlphaMask`, and `AlphaMask` should reuse tile infrastructure rather than duplicate it (DRY principle).

### Root Group Refactor

Change the document root from a bare `Vec<LayerNode>` to a `LayerGroup`:

```rust
pub struct Document {
    pub root: LayerGroup,  // was: pub layers: Vec<LayerNode>
    pub width: u32,
    pub height: u32,
    pub dirty: HashMap<LayerId, DirtyRegion>,
    pub selection: Option<AlphaMask>,
    next_id: LayerId,
}
```

The root group gets a well-known ID (e.g. `LayerId(0)`, pre-allocated). Its `passthrough` is irrelevant (it has no parent to pass through to). All tree operations — flatten, find layer, insert, remove, reorder — become a single recursive implementation on `LayerGroup`. The compositor's entry point is `document.root.flatten()`.

Operations that previously special-cased "no parent" (e.g. add layer at top level) now just use the root group's ID as the parent. The WASM bridge remains unchanged — JS still says `add_raster_layer()` and it goes into the root group by default.

### Generic Tile Storage

Extract the storage, COW, and transaction logic into a generic `TileStore<F>` parameterized by a `TileFormat` trait:

```rust
pub trait TileFormat: 'static + Send + Sync {
    type Data: bytemuck::NoUninit + bytemuck::Zeroable + Clone + Default + Send + Sync;
}

pub struct Rgba;
impl TileFormat for Rgba {
    type Data = [u8; TILE_SIZE * TILE_SIZE * 4]; // 16KB
}

pub struct AlphaF32;
impl TileFormat for AlphaF32 {
    type Data = [f32; TILE_SIZE * TILE_SIZE]; // 16KB (same!)
}

pub type TileGrid = TileStore<Rgba>;
pub type AlphaMask = TileStore<AlphaF32>;
```

`TileStore<F>` retains all existing methods: `get_or_create`, `begin_transaction`, `commit_transaction`, `rollback`, memento system. The type alias `TileGrid = TileStore<Rgba>` keeps all existing code compiling unchanged.

### AlphaMask Operations

On `AlphaMask` specifically — boolean composition and queries only. Shape rasterization
does NOT live here; selection tools use shared rasterization infrastructure to write
into masks, just as paint tools write into layers. The mask is a transparent paint target.

```rust
impl AlphaMask {
    pub fn boolean_add(&mut self, other: &AlphaMask);      // min(1.0, a + b)
    pub fn boolean_subtract(&mut self, other: &AlphaMask);  // max(0.0, a - b)
    pub fn boolean_intersect(&mut self, other: &AlphaMask); // min(a, b)
    pub fn clear(&mut self);
    pub fn invert(&mut self);
    pub fn sample(&self, px: i32, py: i32) -> f32;
    pub fn bounding_rect(&self) -> Option<(i32, i32, i32, i32)>; // tile coords of non-empty
}
```

### Files

- **Modify:** `crates/darkly/src/document.rs` — replace `layers: Vec<LayerNode>` with `root: LayerGroup`, update all tree traversal to use recursive `LayerGroup` methods
- **Modify:** `crates/darkly/src/layer.rs` — add recursive helper methods to `LayerGroup` (flatten, find, insert, remove)
- **Modify:** `crates/darkly/src/gpu/compositor.rs` — update flatten entry point to `document.root`
- **Modify:** `crates/darkly/src/engine.rs` — update layer operations to go through root group
- **Modify:** `frontend/wasm/src/api.rs` — update any direct `layers` access (API surface unchanged)
- **Modify:** `crates/darkly/src/tile.rs` — make generic (`TileStore<F>`, `Tile<F>`, `Memento<F>`)
- **Create:** `crates/darkly/src/mask.rs` — `AlphaMask` boolean ops and utilities (no shape rasterization)
- **Modify:** `crates/darkly/src/undo/tile.rs` — make `TileAction` generic over format, or add `MaskAction`

---

## Phase 2: Tool Overlay System

**Goal:** A GPU render pass that any tool can use to annotate the canvas — lines, handles, shapes, marching ants — in inverted color. Generic and globally usable.

**Why second:** The selection system needs marching ants, which needs the overlay. Building overlay first means selection tools have visual feedback from day one. This also lets us immediately migrate the gradient widget from SVG to GPU.

### What Krita does (reference)

Krita draws all overlays as **actual geometry** — QPainter paths for decorations, native GL `LINE_STRIP` / mitered triangle strips for brush outlines. Never a fullscreen shader. Key details:

- **Marching ants:** Two QPainter stroked paths — white solid underneath, black dashed on top. A 150ms timer advances `dashOffset`. The selection outline is a `QPainterPath` computed asynchronously by `KisUpdateOutlineJob` and cached on `KisSelection`.
- **Brush cursor:** Native GL geometry via `beginNativePainting()`. Uses `GL_FUNC_SUBTRACT` with `(GL_ONE, GL_SRC_COLOR)` blend factors for contrast on any background — produces `src * (1 - dst)`.
- **Transform handles, grids, guides:** QPainter paths with cosmetic pens.
- **Partial updates:** Separate dirty rects for image tiles vs. decorations. `QOpenGLWidget::setUpdateBehavior(PartialUpdate)` preserves the previous frame.

### Our approach: GPU instanced geometry

Krita uses QPainter (CPU) because it has Qt. We don't — we have WebGPU. But the principle is the same: **draw geometry only where primitives are, not evaluate the whole screen.**

We use **instanced rendering** with zero vertex buffers. The primitive storage buffer (already uploaded) is read by both vertex and fragment shaders. Each instance = one primitive. The vertex shader generates a screen-aligned bounding quad per instance. The fragment shader evaluates the SDF for that single primitive within its quad. Total shaded pixels = sum of bounding quad areas, not `screen_width × screen_height`.

### Pipeline position

```
composite_cache → [present] → [veils] → surface → [TOOL OVERLAY on top]
```

The overlay renders **after** present+veils have already written to the surface. It uses `LoadOp::Load` (preserving the canvas content) and draws geometry on top with blend modes. No input texture needed — the overlay doesn't read from the canvas, it just draws on top of it.

This is simpler than the previous design which tried to replace the final blit. Present+veils always run their normal path regardless of whether overlay is active.

### Inversion via blend mode (Krita's approach)

Two pipelines with different blend states, sharing the same shader:

**Solid pipeline** — standard alpha blending:
```
result = src.a * src.rgb + (1 - src.a) * dst.rgb
```

**Invert pipeline** — subtraction blend (same as Krita's `GL_FUNC_SUBTRACT`):
```
result.rgb = src.rgb - dst.rgb * src.rgb = src.rgb * (1 - dst.rgb)
```
Fragment outputs premultiplied white `(alpha, alpha, alpha, alpha)`. On white background → black, on black → white, on mid-gray → proportional contrast. No texture readback needed.

```rust
// wgpu blend state for inversion
BlendState {
    color: BlendComponent {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::Src,    // dst_factor = source color per-channel
        operation: BlendOperation::Subtract,
    },
    alpha: BlendComponent {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::One,
        operation: BlendOperation::Add,
    },
}
```

### Shader (`shaders/overlay.wgsl`)

Bindings — minimal, no input texture:
```wgsl
@group(0) @binding(0) var<uniform> u: OverlayUniforms;
@group(0) @binding(1) var<storage, read> prims: array<OverlayPrimitive>;
```

Vertex shader — generates bounding quad per instance:
```wgsl
@vertex fn vs_main(
    @builtin(vertex_index) vid: u32,     // 0..5 (two triangles)
    @builtin(instance_index) iid: u32,   // primitive index
) -> VertexOutput {
    let prim = prims[iid];
    // Transform canvas-space coords to screen-space if needed
    let p0 = maybe_canvas_to_screen(prim.p0, prim.flags);
    let p1 = maybe_canvas_to_screen(prim.p1, prim.flags);
    // Compute tight bounding box + thickness margin
    let bbox = compute_bbox(prim.kind, p0, p1, prim.thickness);
    // Emit quad corner from vertex index
    let screen_pos = quad_corner(bbox, vid);
    // Screen pixels → NDC
    out.position = vec4f(screen_pos / u.screen_size * 2.0 - 1.0, 0.0, 1.0);
    out.position.y = -out.position.y;
    out.screen_pos = screen_pos;
    out.prim_idx = iid;  // flat interpolation
}
```

Fragment shader — evaluates ONE primitive's SDF:
```wgsl
@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let prim = prims[in.prim_idx];
    let alpha = eval_sdf(prim, in.screen_pos);  // single primitive, not a loop
    // Solid pipeline: return vec4(color.rgb, color.a * alpha)
    // Invert pipeline: return vec4(alpha, alpha, alpha, alpha)  (premultiplied white)
}
```

Two entry points (`fs_solid`, `fs_invert`) or one entry point that always outputs appropriately for its pipeline. Since primitives are sorted by blend mode before drawing, each draw call uses one pipeline.

### Data structures (unchanged public API)

```rust
// 64 bytes, std430-aligned — same struct as before
#[repr(C)]
struct OverlayPrimitive {
    color: [f32; 4],
    p0: [f32; 2],
    p1: [f32; 2],
    thickness: f32,
    dash_len: f32,
    dash_offset: f32,
    corner_radius: f32,
    kind: u32,        // 0=line, 1=circle, 2=rect, 3=dashed_line, 4=filled_rect, 5=filled_circle
    flags: u32,       // bit0=canvas_space, bit1=invert_color
    _pad: [u32; 2],
}

// Simplified uniforms — no input texture, no mask, no inverse transform
struct OverlayUniforms {
    screen_size: [f32; 2],
    time: f32,
    _pad: f32,
    fwd_row0: [f32; 4],  // canvas → screen transform
    fwd_row1: [f32; 4],
    fwd_row2: [f32; 4],
}
```

### Integration

The overlay is a separate pass after `present_and_veils`, not woven into it:

```rust
// In compositor's render() / present_only():
self.present_and_veils(device, queue, &mut encoder, &surface_view);  // unchanged
if self.tool_overlay.has_content() {
    self.tool_overlay.encode(device, queue, &mut encoder, &surface_view, &vt, vw, vh);
}
```

`present_and_veils` reverts to its clean form — no overlay branching, no direct_mode. The overlay pass uses `LoadOp::Load` on the surface and draws instanced geometry with the appropriate blend pipeline.

Internally, `encode()` partitions primitives into solid vs. inverted, uploads the storage buffer once, and issues up to two draw calls:
```rust
fn encode(&mut self, ..., surface_view: &TextureView, ...) {
    // Upload primitives to storage buffer (sorted: solid first, then inverted)
    // Begin render pass with LoadOp::Load on surface_view
    // Draw solid primitives: set solid_pipeline, draw(0..6, 0..solid_count)
    // Draw inverted primitives: set invert_pipeline, draw(0..6, solid_count..total_count)
}
```

### Marching ants (deferred to Phase 3)

Marching ants follow Krita's model: CPU-side contour extraction produces a polyline, submitted as overlay dashed-line primitives with animated `dash_offset`. No fullscreen edge detection shader. The contour is recomputed only when the selection mask changes (infrequent), not every frame. The overlay system just sees dashed lines — it doesn't know they're marching ants.

### Engine API (unchanged)

```rust
pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>);
pub fn clear_overlay(&mut self);
pub fn set_overlay_mask(&mut self, mask: &AlphaMask);  // for future marching ants
pub fn clear_overlay_mask(&mut self);
pub fn update_overlay_time(&mut self, dt: f32);
pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize>;
```

### WASM bridge (unchanged)

```rust
pub fn set_overlay(&mut self, primitives_json: JsValue);
pub fn clear_overlay(&mut self);
pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize>;
```

### Hit testing (unchanged)

CPU-side SDF evaluation on the primitive list. No GPU involvement.

### Files

- **Rewrite:** `crates/darkly/src/gpu/overlay.rs` — instanced geometry renderer, two blend pipelines, sorted draw calls
- **Rewrite:** `shaders/overlay.wgsl` — vertex shader generates bounding quads, fragment evaluates single-primitive SDF
- **Modify:** `crates/darkly/src/gpu/compositor.rs` — move overlay call after `present_and_veils`, remove overlay branching from `present_and_veils`
- **No change:** `crates/darkly/src/engine.rs`, `frontend/wasm/src/api.rs` — public API is identical
- **Migrate (later):** `frontend/src/tools/gradient.svelte.ts` — migrate from SVG to GPU overlay after system is verified working

---

## Phase 3: Selection System

**Goal:** Document-level selection with masked tile writing and rectangle select tool.

### Design

```rust
// In Document
pub selection: Option<AlphaMask>,
```

### Selection-Aware Tile Writing

The key integration point. In `Document`, the paint/erase/fill operations currently write directly to tiles. We add a single masking step in the shared write path:

```rust
// Before writing a dab to a tile:
if let Some(sel) = &self.selection {
    if let Some(mask_tile) = sel.get(tx, ty) {
        // Multiply dab alpha by selection alpha
        for each pixel in dab {
            dab_alpha *= mask_tile.sample(px, py);
        }
    } else {
        // No mask tile = fully unselected, skip write entirely
        continue;
    }
}
```

This lives in the tile write path, not in individual tools. Every tool gets selection masking for free.

### Marching Ants

When the selection changes, extract the contour polyline from the `AlphaMask` on the CPU (Krita does this asynchronously via `KisUpdateOutlineJob`; we do it synchronously since masks are small). The contour is submitted as overlay dashed-line primitives with `dash_offset` animated by a timer. The overlay system renders them as ordinary geometry — it doesn't know they're marching ants.

### Rectangle Select Tool

First tool, follows the auto-discovery pattern:

```rust
// crates/darkly/src/tools/rect_select.rs
pub fn register() -> ToolRegistration { ... }
```

The tool:
1. Receives mouse down/move/up events
2. Computes rectangle in canvas coordinates
3. Rasterizes the rectangle into a temporary `AlphaMask` using shared rasterization infrastructure, then applies it to the document selection via boolean ops (or replace)
4. Submits overlay primitives for the selection rectangle preview during drag
5. After mouse up, selection is committed and marching ants appear

Boolean modifiers:
- No modifier: replace selection
- Shift: add (boolean_add)
- Alt: subtract (boolean_subtract)
- Shift+Alt: intersect (boolean_intersect)

### Undo

Selection changes are undoable. `SelectionAction` wraps a `Memento<AlphaF32>` — same mechanism as tile undo, just operating on the selection's `AlphaMask`.

### Files

- **Modify:** `crates/darkly/src/document.rs` — add `selection` field, add masking to tile write path
- **Create:** `crates/darkly/src/tools/rect_select.rs` — rectangle select tool
- **Modify:** `crates/darkly/src/undo/` — add `SelectionAction`
- **Modify:** `crates/darkly/src/engine.rs` — selection CRUD, wire to overlay mask
- **Modify:** `frontend/wasm/src/api.rs` — selection API
- **Modify:** `crates/darkly/build.rs` — enable tool auto-discovery (currently commented out)

---

## Phase 4: More Selection Tools

**Goal:** Ellipse, lasso, magic wand.

Each is a standalone file in `crates/darkly/src/tools/`, auto-discovered by `build.rs`.

- **Ellipse select:** Rasterizes an ellipse into a temporary mask using shared rasterization, then applies via boolean ops — same modifiers as rect
- **Lasso select:** Rasterizes a closed polygon (scanline fill) into a temporary mask, then applies via boolean ops
- **Magic wand:** Flood fill variant that writes to the selection `AlphaMask` instead of the layer. Tolerance-based, samples from composite cache (GPU readback needed here — same technique as color picker)

The rasterization infrastructure (rect fill, ellipse fill, polygon scanline) is shared across tools and paint targets. Selection tools use it to write into `AlphaMask`; paint tools use it to write into `TileGrid`. The mask itself has no knowledge of shapes.

### Files

- **Create:** `crates/darkly/src/tools/ellipse_select.rs`
- **Create:** `crates/darkly/src/tools/lasso_select.rs`
- **Create:** `crates/darkly/src/tools/magic_wand.rs`

---

## Phase 5: Layer Masks

**Goal:** Non-destructive per-layer alpha masks. Paintable. Convertible to/from selection.

### Design

Both `RasterLayer` and `LayerGroup` get a mask — Photoshop-style, one per node:

```rust
pub struct RasterLayer {
    pub id: LayerId,
    pub name: String,
    pub tiles: TileGrid,
    pub mask: Option<AlphaMask>,   // NEW
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    pub mask_visible: bool,        // NEW — can toggle mask effect
}

pub struct LayerGroup {
    pub id: LayerId,
    pub name: String,
    pub children: Vec<LayerNode>,
    pub mask: Option<AlphaMask>,   // NEW — masks the entire group's output
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    pub mask_visible: bool,        // NEW
    pub passthrough: bool,
    pub collapsed: bool,
}
```

A group mask multiplies the group's composited output before blending into the parent. This requires compositing group children into a temporary buffer (needed anyway for non-passthrough groups), then applying the mask to that buffer.

### Compositor Integration

The composite shader gains an optional mask texture binding:

```wgsl
// composite.wgsl addition
@group(0) @binding(4) var t_mask: texture_2d<f32>;
@group(0) @binding(5) var<uniform> has_mask: u32;

// In fragment:
var mask_alpha = 1.0;
if (has_mask != 0u) {
    mask_alpha = textureSample(t_mask, s, uv).r;
}
final_alpha = fg.a * opacity * mask_alpha;
```

The compositor uploads the mask tile grid to a GPU texture (same upload path as layer tiles) and binds it during the blend pass for that layer. For groups, the mask is applied after the group's children are composited into the temporary buffer, before blending the group result into the parent accumulator.

### Painting on Masks

When the user targets a mask for painting, the engine redirects tile writes to `layer.mask` instead of `layer.tiles`. The brush writes f32 values (0.0 = transparent, 1.0 = opaque) using the same write path. The mask is just another painting surface.

### Mask ↔ Selection Conversion

- **Selection → Mask:** `layer.mask = document.selection.clone()`
- **Mask → Selection:** `document.selection = layer.mask.clone()`
- Both are `AlphaMask`, so conversion is a data copy.

### Files

- **Modify:** `crates/darkly/src/layer.rs` — add `mask`, `mask_visible` fields
- **Modify:** `crates/darkly/src/gpu/compositor.rs` — upload mask texture, bind during blend pass
- **Modify:** `shaders/composite.wgsl` — sample mask, multiply alpha
- **Modify:** `crates/darkly/src/document.rs` — mask paint targeting, mask↔selection conversion
- **Modify:** `crates/darkly/src/engine.rs` — mask CRUD API
- **Modify:** `frontend/wasm/src/api.rs` — mask API (add/remove/target/convert)

---

## Phase 6: Copy/Paste

**Goal:** Copy selected region, paste as new layer.

### Design

```rust
// In Engine
clipboard: Option<ClipboardBuffer>,

pub struct ClipboardBuffer {
    tiles: TileGrid,           // RGBA pixel data
    offset: (i32, i32),        // canvas position hint for paste
    width: u32, height: u32,   // bounds of copied region
}
```

### Copy

1. Flatten visible layers within selection bounds (merge composited output)
2. For each tile in the merged result, multiply by selection alpha
3. Store in `ClipboardBuffer` with position = selection bounding rect origin

For "copy from active layer only" (non-merged): read directly from the active layer's tiles.

### Paste

1. Create new `RasterLayer` from clipboard tiles
2. Position at clipboard offset (or center of viewport)
3. Add to layer stack above active layer
4. Select the new layer
5. Auto-activate transform tool (so user can reposition)

### Browser Clipboard

JS-side: encode `ClipboardBuffer` as PNG blob via canvas, write to `navigator.clipboard`. On paste from external source: decode PNG, upload tiles via WASM bridge. This is decoupled from the Rust core.

### Files

- **Create:** `crates/darkly/src/clipboard.rs` — `ClipboardBuffer`, copy/paste logic
- **Modify:** `crates/darkly/src/engine.rs` — copy/paste API
- **Modify:** `frontend/wasm/src/api.rs` — copy/paste bridge
- **Modify:** `frontend/src/` — keyboard shortcuts (Ctrl+C/V/X), browser clipboard integration

---

## Phase 7: Interactive Transforms

**Goal:** GPU-side preview of scale/rotate/skew/flip with transform handles rendered via the tool overlay.

### Design

When the user activates the transform tool on a layer:

1. **Snapshot:** The layer's current tiles are uploaded to a dedicated GPU texture (`transform_source`)
2. **Clear layer:** The layer's tiles are temporarily cleared (or hidden from compositing)
3. **Render transformed:** A transform shader renders `transform_source` through a matrix, writing to a temporary texture that the compositor blends in place of the original layer
4. **Handles:** Transform bounding box + handles rendered via tool overlay primitives

Each frame, the user drags handles → JS computes new transform matrix → uploads matrix uniform → compositor renders transformed preview.

### Transform Matrix

```rust
pub struct InteractiveTransform {
    source_texture: Texture,
    source_bounds: (i32, i32, u32, u32),  // original position/size
    matrix: [f32; 6],                       // 2D affine (2x3)
    interpolation: Interpolation,           // Bilinear / Bicubic
}
```

### Shader (`shaders/transform.wgsl`)

Samples `transform_source` with inverse-transformed UVs. Bilinear interpolation built-in via sampler; bicubic requires manual 4x4 tap.

### Transform Handles via Overlay

The transform tool submits overlay primitives:
- 1 rect (bounding box, dashed, canvas-space)
- 8 circles (corner + midpoint handles, screen-space — fixed size)
- 1 circle (rotation handle, screen-space)
- 1 line (rotation arm, canvas-space)

All in inverted color. Hit testing determines which handle is grabbed.

### Files

- **Create:** `crates/darkly/src/gpu/transform.rs` — `InteractiveTransform`, GPU resources, shader integration
- **Create:** `shaders/transform.wgsl` — affine transform sampling
- **Modify:** `crates/darkly/src/gpu/compositor.rs` — render transform preview in layer's place
- **Create:** `crates/darkly/src/tools/transform.rs` — transform tool (handle logic, matrix computation)
- **Modify:** `frontend/wasm/src/api.rs` — transform API
- **Create:** `frontend/src/tools/transform.svelte.ts` — transform tool UI

---

## Phase 8: Transform Commit (GPU Readback)

**Goal:** Rasterize the transformed result back to tiles.

### Flow

1. User confirms transform (Enter key or click away)
2. Engine renders final transformed texture at full resolution
3. `map_async` on a staging buffer to read pixels back to CPU
4. Write pixels into layer tiles (new transaction for undo)
5. Clean up transform GPU resources
6. Mark affected tiles dirty

### GPU Readback

```rust
// Render transformed texture at document resolution
// Copy texture → staging buffer
staging_buffer.slice(..).map_async(MapMode::Read, callback);
device.poll(Maintain::Wait); // or async in WASM
let data = staging_buffer.slice(..).get_mapped_range();
// Write data into tiles
```

This is async on WebGPU. The engine enters a "committing transform" state where it awaits the readback before allowing further edits.

### Undo

The commit creates a `TileAction` with mementos of all affected tiles. Undoing restores the original tiles and re-enters transform mode (or just restores tiles).

### Files

- **Modify:** `crates/darkly/src/gpu/transform.rs` — add commit/readback logic
- **Modify:** `crates/darkly/src/engine.rs` — commit flow, async readback handling

---

## Phase 9: Warp Transforms

**Goal:** Mesh-based and displacement-based warping.

### Mesh Warp (Perspective/Envelope)

- Subdivide the source texture's bounding box into an NxN grid of quads
- Each quad vertex has a position and UV
- User drags control points → vertex positions change → mesh deforms
- GPU renders the mesh with the source texture mapped via UVs
- Same commit path as Phase 8

The vertex buffer is uploaded each frame when dirty. The shader is a standard textured mesh shader with interpolation.

### Displacement Warp (Liquify)

- A 2-channel displacement texture (`RG32Float`) stores per-pixel dx/dy offsets
- The liquify brush writes into this displacement texture
- The transform shader samples: `color = source.sample(uv + displacement.sample(uv).rg)`
- Same commit path — the displaced result is rasterized back to tiles

### Files

- **Create:** `crates/darkly/src/gpu/warp.rs` — mesh warp vertex buffer, displacement texture
- **Create:** `shaders/warp.wgsl` — mesh UV shader, displacement sampling
- **Create:** `crates/darkly/src/tools/warp.rs` — warp tool (control points, mesh editing)
- **Create:** `crates/darkly/src/tools/liquify.rs` — liquify brush tool

---

## Dependency Graph

```
Phase 1 (AlphaMask) ──→ Phase 3 (Selection) ──→ Phase 4 (More Tools)
                    ├──→ Phase 5 (Layer Masks)
                    └──→ Phase 6 (Copy/Paste)

Phase 2 (Tool Overlay) ──→ Phase 3 (marching ants)
                        └──→ Phase 7 (transform handles)

Phase 7 (Transforms) ──→ Phase 8 (Commit) ──→ Phase 9 (Warp)
```

Phases 1 and 2 can be built in parallel. After those, phases 3–6 are independently buildable. Phases 7–9 are sequential.

---

## Verification

After each phase, verify:

1. **Phase 1:** Unit test — create `AlphaMask`, fill rect, boolean add/subtract, verify values. Ensure existing `TileGrid` (now `TileStore<Rgba>`) compiles and passes all existing tests.
2. **Phase 2:** Visual — use `overlay_debug` POC tool to render primitives (line, circle, rect) on canvas. Verify inverted color contrast on light/dark backgrounds. Verify no performance regression (should be instant, not laggy). Verify no CPU spike when primitives are static.
3. **Phase 3:** Visual — draw rectangle selection, paint across boundary, verify clipping. Verify marching ants animate. Test boolean modifiers.
4. **Phase 4:** Visual — each selection tool produces correct mask shape.
5. **Phase 5:** Visual — add mask to layer, paint on mask, verify transparency. Convert mask↔selection.
6. **Phase 6:** Functional — copy selection, paste, verify new layer content matches.
7. **Phase 7:** Visual — activate transform, drag handles, verify preview updates in real-time.
8. **Phase 8:** Functional — commit transform, verify tiles contain transformed pixels, undo restores original.
9. **Phase 9:** Visual — drag warp grid, verify mesh deformation. Liquify brush displaces pixels.

Build verification: `cargo build --target wasm32-unknown-unknown` after each phase. Run `wasm-pack build` and test in browser.
