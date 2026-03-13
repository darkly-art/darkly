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
    pub fn rasterize(&mut self, bounds, sdf_fn, antialias, feather); // SDF → coverage → tiles
    pub fn feather(&mut self, radius: f32);                          // separable Gaussian blur
}
```

### Shared SDF & Rasterization Infrastructure

A shared `sdf` module provides pure signed distance functions used by both the GPU overlay (WGSL) and CPU mask rasterization (Rust). The same mathematical formulas appear in both contexts — one is the source of truth for the other.

```rust
// crates/darkly/src/sdf.rs — pure SDF functions

/// Unsigned distance from point to line segment.
pub fn sdf_segment(px, py, ax, ay, bx, by) -> f32;

/// Signed distance to filled circle (negative inside).
pub fn sdf_circle(px, py, cx, cy, r) -> f32;

/// Signed distance to filled axis-aligned rectangle (negative inside).
pub fn sdf_rect(px, py, cx, cy, half_w, half_h) -> f32;

/// Signed distance to filled rounded rectangle (negative inside).
pub fn sdf_rounded_rect(px, py, cx, cy, half_w, half_h, corner_r) -> f32;

/// Signed distance to filled ellipse (negative inside).
/// Uses implicit surface approximation — exact on boundary, accurate within ±1px.
pub fn sdf_ellipse(px, py, cx, cy, rx, ry) -> f32;

/// Signed distance to filled polygon (negative inside).
/// Uses winding-number edge-following (Inigo Quilez algorithm).
pub fn sdf_polygon(px, py, vertices: &[[f32; 2]]) -> f32;

/// Convert SDF to alpha coverage.
///   antialias=true:  smoothstep over 1px transition (same as overlay shader)
///   antialias=false: binary 0/1 at boundary
///   feather > 0:     smoothstep over feather-width transition
pub fn sdf_coverage(sdf, antialias, feather) -> f32;
```

**Antialiasing** is a property of the SDF→coverage conversion, not the SDF itself. The coverage function provides three modes:

- **Hard edge** (`antialias=false, feather=0`): binary 0/1. For pixel art and precise masking.
- **Antialiased** (`antialias=true, feather=0`): `smoothstep` over 1px. Identical to the overlay shader's approach. Produces analytically exact subpixel coverage — no supersampling needed.
- **Feathered** (`feather > 0`): `smoothstep` over `feather` pixels. Tool-level feathering is free during rasterization — just widen the SDF transition band, no separate blur pass needed for geometric selections.

`AlphaMask::rasterize()` iterates tiles within the shape's bounding rect (plus margin for AA/feather), evaluates the SDF at each pixel center, converts to coverage, and writes non-zero values. Selection tools provide the SDF as a closure:

```rust
// Rectangle selection tool
mask.rasterize(bounds, |px, py| sdf::sdf_rect(px, py, cx, cy, hw, hh), antialias, feather);

// Ellipse selection tool
mask.rasterize(bounds, |px, py| sdf::sdf_ellipse(px, py, cx, cy, rx, ry), antialias, feather);
```

**Standalone feathering** (`AlphaMask::feather()`) handles non-geometric masks — hand-painted masks, magic wand results, or "feather existing selection" menu actions. These don't have an SDF, so a separable Gaussian blur on f32 tiles is the right approach: horizontal pass then vertical pass, O(r) per pixel via 1D kernel.

The overlay system's CPU hit-test code (`overlay.rs`) is refactored to use the shared SDF functions from `sdf.rs`, eliminating the duplicated `cpu_sdf` / `sdf_line_cpu` implementations.

### Files

- **Modify:** `crates/darkly/src/document.rs` — replace `layers: Vec<LayerNode>` with `root: LayerGroup`, update all tree traversal to use recursive `LayerGroup` methods
- **Modify:** `crates/darkly/src/layer.rs` — add recursive helper methods to `LayerGroup` (flatten, find, insert, remove)
- **Modify:** `crates/darkly/src/gpu/compositor.rs` — update flatten entry point to `document.root`
- **Modify:** `crates/darkly/src/engine.rs` — update layer operations to go through root group
- **Modify:** `frontend/wasm/src/api.rs` — update any direct `layers` access (API surface unchanged)
- **Modify:** `crates/darkly/src/tile.rs` — make generic (`TileStore<F>`, `Tile<F>`, `Memento<F>`)
- **Create:** `crates/darkly/src/mask.rs` — `AlphaMask` boolean ops and utilities (no shape rasterization)
- **Create:** `crates/darkly/src/sdf.rs` — shared SDF functions, coverage conversion
- **Modify:** `crates/darkly/src/gpu/overlay.rs` — refactor `cpu_sdf` to use shared `sdf.rs` functions
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

The overlay renders **after** present+veils have already written to the surface. It uses `LoadOp::Load` (preserving the canvas content) and draws geometry on top. Present+veils always run their normal path regardless of whether overlay is active.

### Inversion via snapshot-based luminance threshold

Two pipelines sharing the same shader, both using standard premultiplied alpha blending:

**Solid pipeline** (`fs_solid`) — outputs premultiplied color directly from the primitive's `color` field.

**Invert pipeline** (`fs_invert`) — samples a snapshot of the surface (GPU-to-GPU copy taken just before the overlay pass), computes greyscale luminance, and thresholds at 0.5: white on dark backgrounds, black on light backgrounds. Always greyscale output.

Why not pure blend math? A subtraction blend (`src * (1 - dst)`) can invert per-channel without reading the framebuffer, but it produces complementary colors (cyan on red, magenta on green) rather than greyscale, and transitions gradually rather than with a hard threshold. The luminance threshold requires a cross-channel dot product and a step function, which the fixed-function blend unit cannot express — it only does per-channel multiply/add/subtract. The snapshot gives the fragment shader access to the existing canvas pixel so it can do the math itself.

The snapshot copy is cheap (GPU-to-GPU memcpy, microseconds) and is skipped entirely when no inverted primitives are present. Memory cost is one viewport-sized texture (~8MB at 1920×1080).

### Shader (`shaders/overlay.wgsl`)

Bindings:
```wgsl
@group(0) @binding(0) var<uniform> u: OverlayUniforms;
@group(0) @binding(1) var<storage, read> prims: array<OverlayPrimitive>;
@group(0) @binding(2) var t_snapshot: texture_2d<f32>;  // surface copy for invert
@group(0) @binding(3) var t_sampler: sampler;
```

Two fragment entry points:
```wgsl
@fragment fn fs_solid(in: VertexOutput) -> @location(0) vec4f {
    let alpha = eval_prim(prim, in.screen_pos);
    let a = prim.color.a * alpha;
    return vec4f(prim.color.rgb * a, a);  // premultiplied color
}

@fragment fn fs_invert(in: VertexOutput) -> @location(0) vec4f {
    let alpha = eval_prim(prim, in.screen_pos);
    let bg = textureSampleLevel(t_snapshot, t_sampler, uv, 0.0).rgb;
    let lum = dot(bg, vec3f(0.2126, 0.7152, 0.0722));
    let rgb = select(vec3f(0.0), vec3f(1.0), lum < 0.5);
    let a = prim.color.a * alpha;
    return vec4f(rgb * a, a);  // greyscale threshold
}
```

Primitives are sorted by `FLAG_INVERT_COLOR` before upload — solid first, then inverted. `encode()` issues up to two draw calls, one per pipeline.

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

- **Rewrite:** `crates/darkly/src/gpu/overlay.rs` — instanced geometry renderer, two pipelines (solid + invert with snapshot), sorted draw calls
- **Rewrite:** `shaders/overlay.wgsl` — vertex shader generates bounding quads, `fs_solid` and `fs_invert` entry points
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

**Threshold for soft selections:** Antialiased and feathered masks have no single crisp boundary. The contour extraction thresholds at 0.5 — pixels with coverage > 0.5 are "inside" for marching ants purposes. This matches standard practice (Krita, Photoshop).

### Rectangle Select Tool

First tool, follows the auto-discovery pattern:

```rust
// crates/darkly/src/tools/rect_select.rs
pub fn register() -> ToolRegistration { ... }
```

The tool:
1. Receives mouse down/move/up events
2. Computes rectangle in canvas coordinates
3. Rasterizes the rectangle into a temporary `AlphaMask` via `mask.rasterize(bounds, |px, py| sdf::sdf_rect(...), antialias, feather)`, then applies it to the document selection via boolean ops (or replace)
4. Submits overlay primitives for the selection rectangle preview during drag
5. After mouse up, selection is committed and marching ants appear

Tool options (same for all selection tools):
- **Anti-aliased** (bool, default true): smooth 1px edge transition vs binary. Disabled automatically when feather > 0 (feathering subsumes AA). For pixel art or precise masking, uncheck.
- **Feather** (f32, default 0): smooth transition radius in pixels. Applied during SDF rasterization — no separate blur pass needed for geometric selections.

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

## Phase 6: Copy/Paste ✅

**Goal:** Copy selected region, paste as new layer. Seamless system clipboard integration for external images (screenshots, web images) — zero prompts.

### Architecture

Two clipboard channels:

1. **Internal clipboard** — `Clipboard` enum holding typed content (extensible for future layer/group copying). Phase 6 implements `ImageData(ImageClip)` variant.
2. **System clipboard** — PNG blob via browser `navigator.clipboard` API. Written on every Copy/Cut (async). Read on every Paste.

All image decode/encode happens in JS via Canvas API (`createImageBitmap` + `OffscreenCanvas`) — handles any browser-supported format (PNG, JPEG, WebP, GIF, BMP, SVG, AVIF, ICO). Always sRGB RGBA8, no color profile ambiguity, no prompts.

### Operations

- **Copy (Ctrl+C):** Reads active layer tiles within selection bounds (or entire layer if no selection), multiplies by selection alpha, stores in internal `Clipboard::ImageData` + pushes PNG to system clipboard via JS.
- **Cut (Ctrl+X):** Copy + `clear_selection_contents` (already implemented/undoable).
- **Paste (Ctrl+V):** JS reads system clipboard → decodes any image format via `createImageBitmap` → raw RGBA to WASM → creates new "Pasted Layer" above active layer at document center.
- **Paste in Place (Ctrl+Shift+V):** Uses internal clipboard at original copy position.
- **Copy Merged (Ctrl+Shift+C):** Deferred to Phase 8 (requires GPU readback for correct blend-mode compositing).

### Undo

- Copy: no state change → no undo needed.
- Cut: reuses existing `clear_selection_contents` transaction → already undoable.
- Paste: `add_raster_layer` pushes `LayerAddAction` → undo removes pasted layer.

### Files

- **Created:** `crates/darkly/src/clipboard.rs` — `Clipboard` enum, `ImageClip` struct, `from_layer`, `from_rgba`, `to_rgba`, `write_to_layer`
- **Modified:** `crates/darkly/src/engine.rs` — `clipboard` field, `copy`, `cut`, `paste_image`, `paste_in_place`, `ClipboardExport`
- **Modified:** `frontend/wasm/src/api.rs` — WASM bridge for copy/cut/paste_image/paste_in_place
- **Created:** `frontend/src/clipboard.ts` — `copyToSystemClipboard`, `readImageFromClipboard` (OffscreenCanvas + Clipboard API)
- **Modified:** `frontend/src/editor.ts` — hotkey wiring for copy/cut/paste/pasteInPlace
- **Modified:** `crates/darkly/src/config.rs` — hotkey defaults for copy/cut/paste/pasteInPlace
- **Modified:** `crates/darkly/src/lib.rs` — `pub mod clipboard`

---

## Phase 7: Floating Content, Interactive Transforms & Paste-in-Place

**Goal:** A unified floating content system for GPU-previewed transforms and paste-in-place. Paste-in-place (Ctrl+Shift+V) places content as a transformable floating preview on the current layer or mask — not as a new layer. Press Enter to commit, Escape to cancel. The transform tool extracts layer content into the same floating system. This consolidates the old Phases 7 and 8 into a single phase.

Ctrl+V behavior is unchanged (creates a new layer).

### Core Concept: FloatingContent

A `FloatingContent` holds source pixels (CPU tiles for commit + GPU texture for preview), an affine transform matrix, and a reference to the target layer/mask. Two entry points create one:

1. **Paste-in-place (Ctrl+Shift+V)** — clipboard content → floating on current layer/mask at original copy position
2. **Transform tool** — extract current layer/mask content → floating, clear originals

Both share the same GPU preview pipeline, transform handles, commit, and cancel flow.

### Data Structures

```rust
// crates/darkly/src/gpu/transform.rs

pub struct FloatingContent {
    // CPU-side source (for commit — avoids async GPU readback)
    source_tiles: TileGrid,             // always RGBA, even for mask sources
    source_origin: (i32, i32),          // pixel offset in document space

    // GPU-side source (for real-time preview)
    source_texture: wgpu::Texture,
    source_view: wgpu::TextureView,
    source_width: u32,
    source_height: u32,

    // Transform state
    matrix: [f32; 6],                   // 2D affine (2×3), starts as identity
    interpolation: Interpolation,

    // Target
    target_layer: LayerId,
    target_is_mask: bool,

    // Determines commit/cancel behavior
    mode: FloatingMode,
}

pub enum FloatingMode {
    /// Clipboard paste — commit composites INTO target. Cancel = no-op.
    Paste,
    /// Extracted from layer — commit writes transformed pixels.
    /// Cancel restores original tiles from stored memento.
    Transform { original_memento: TransactionMemento },
}

pub enum Interpolation { Bilinear, Bicubic }
```

### GPU Preview

#### Transform-blend shader (`shaders/transform.wgsl`)

A single-pass fullscreen-triangle fragment shader that reads the compositor accumulator as background, samples the source texture through the inverse affine matrix, and blends — no intermediate texture needed:

```wgsl
struct TransformBlendUniforms {
    inv_matrix: mat3x3<f32>,       // inverse affine for source UV lookup
    source_origin: vec2f,          // document-space pixel offset
    source_size: vec2f,            // source texture dimensions
    canvas_size: vec2f,            // full canvas dimensions
    opacity: f32,
    target_is_mask: u32,
}

@fragment fn fs_main(in: VertexOut) -> @location(0) vec4f {
    let bg = textureLoad(t_bg, vec2i(in.position.xy), 0);
    let canvas_pos = in.uv * u.canvas_size;
    let local_pos = canvas_pos - u.source_origin;
    let src_pos = (u.inv_matrix * vec3f(local_pos, 1.0)).xy;
    let src_uv = src_pos / u.source_size;

    if (any(src_uv < vec2f(0.0)) || any(src_uv >= vec2f(1.0))) {
        return bg;
    }

    let fg = textureSampleLevel(t_source, s_source, src_uv, 0.0);
    let a = fg.a * u.opacity;
    // Normal blend (premultiplied alpha)
    return vec4f(fg.rgb * u.opacity + bg.rgb * (1.0 - a), a + bg.a * (1.0 - a));
}
```

Bilinear interpolation via the GPU sampler; bicubic deferred to a later enhancement.

#### Compositor integration

In `render_offscreen`, after compositing a raster layer, if that layer has floating content, insert an extra ping-pong blend pass using the transform-blend pipeline:

```rust
Layer::Raster(raster) => {
    // ... existing blend pass for the layer ...

    // If this layer has floating content, composite it on top
    if let Some(fc) = &self.floating_content {
        if fc.target_layer == raster.id {
            let src = self.current_accum;
            let dst = if is_last_layer { /* cache */ } else { 1 - src };
            self.current_accum = dst;
            // render pass with transform_blend_pipeline + fc.bind_group
        }
    }
}
```

For **transform mode**: the layer's tiles are cleared, so the first blend pass produces nothing visible. The floating pass renders the transformed content. Net visual: content appears transformed.

For **paste mode**: the layer renders normally. The floating pass overlays pasted content on top. Net visual: existing layer + pasted content.

For **mask-targeted content**: when `show_mask` is active, the floating content renders as part of the mask grayscale visualization. When compositing normally, it's applied as additional mask data (multiplied into layer alpha).

### Transform Handles (via tool overlay)

When floating content is active, overlay primitives are submitted:
- 1 dashed rect: bounding box (canvas-space, inverted color)
- 4 filled circles: corner handles (screen-space, fixed 5px radius)
- 4 filled circles: edge midpoint handles (screen-space, fixed 4px radius)
- 1 filled circle: rotation handle above top-center (screen-space, 5px radius)
- 1 line: rotation arm from top-center to rotation handle (canvas-space)

All use `FLAG_INVERT_COLOR`. Hit testing determines which handle is grabbed.

Handle interactions (in `crates/darkly/src/tools/transform.rs`):
- **Corner drag**: scale from opposite corner as anchor
- **Edge drag**: scale along one axis from opposite edge
- **Rotation handle drag**: rotate around center
- **Interior drag**: translate (move)
- **Shift + rotation**: snap to 15° increments
- **Shift + scale**: maintain aspect ratio

Matrix computation: translate to anchor → scale/rotate → translate back.

### Paste-in-Place Flow (Ctrl+Shift+V)

1. JS reads internal clipboard; if none, tries system clipboard with stored offset
2. WASM bridge calls `engine.paste_in_place_floating(active_layer_id)`
3. Engine creates `FloatingContent`:
   - Source = clipboard ImageClip tiles + bounds
   - Target = current layer (or mask if `editing_mask_layer` is set)
   - Mode = Paste, matrix = identity, origin = clipboard stored offset
4. Compositor renders floating content overlaid on target layer
5. Transform handles appear — user can move, scale, rotate
6. Enter → commit, Escape → cancel

### Transform Tool Flow

1. User activates transform tool on a layer (or mask if editing mask)
2. Engine creates `FloatingContent`:
   - Clones source tiles within bounds (selection bounds or entire layer)
   - Begins transaction, clears tiles within bounds, commits → stores memento
   - Uploads source tiles to GPU texture
   - Mode = Transform { original_memento }
3. Compositor renders floating content where cleared tiles were
4. Transform handles appear
5. Enter → commit, Escape → cancel (restores original tiles via memento)

### Commit Flow (Enter)

CPU-side affine rasterization — source data is in `source_tiles`, no GPU readback needed:

1. Compute output bounding box from transformed source corners
2. For each pixel in output bounds: apply inverse matrix → bilinear sample from `source_tiles` → write to target
   - **Paste mode**: Normal blend onto existing tiles/mask, respecting selection
   - **Transform mode**: direct write (tiles were already cleared)
   - **Mask target**: extract luminance (`0.2126*r + 0.7152*g + 0.0722*b`) from RGBA, write as f32 alpha
3. Push `TileAction` with mementos of all affected tiles. Ctrl+Z restores pre-commit state.

### Cancel Flow (Escape)

- **Paste mode**: discard FloatingContent — target unchanged, no undo step.
- **Transform mode**: undo the tile-clear via stored `original_memento`, restoring original tiles. No undo step.

### Auto-Commit

Floating content auto-commits when the user:
- Switches active layer
- Activates a different tool
- Starts any paint/fill/erase operation
- Triggers undo/redo

### Mask ↔ Layer Cross-Paste

**Copy from mask** (when `editing_mask_layer` is set):
- `ImageClip::from_mask(mask, selection)` converts f32 alpha → grayscale RGBA: `[v, v, v, 255]` where `v = (alpha * 255).round()`
- Stored in internal clipboard as normal RGBA ImageClip

**Paste into mask** (commit-time conversion):
- Luminance extraction: `alpha = 0.2126*r + 0.7152*g + 0.0722*b`
- Written as f32 to AlphaMask tiles

**Paste into layer** (from mask-sourced clipboard): grayscale RGBA composites normally.

### Files

- **Create:** `crates/darkly/src/gpu/transform.rs` — `FloatingContent`, `FloatingMode`, GPU texture management, transform-blend pipeline, bind group, `rasterize_to()` CPU commit
- **Create:** `shaders/transform.wgsl` — affine transform + blend fragment shader
- **Create:** `crates/darkly/src/tools/transform.rs` — transform tool registration, handle hit-test, matrix computation
- **Modify:** `crates/darkly/src/gpu/compositor.rs` — store `Option<FloatingContent>`, add transform-blend pass in `render_offscreen`, create pipeline in `new()`
- **Modify:** `crates/darkly/src/engine.rs` — `floating_content` field, `paste_in_place_floating()`, `begin_transform()`, `commit_floating()`, `cancel_floating()`, auto-commit hooks, copy-from-mask
- **Modify:** `crates/darkly/src/clipboard.rs` — `ImageClip::from_mask()` for mask→RGBA copy
- **Modify:** `frontend/wasm/src/api.rs` — WASM bridge: floating paste, commit, cancel, update_transform_matrix, begin_transform
- **Modify:** `frontend/src/editor.ts` — Ctrl+Shift+V → floating paste, Enter/Escape handlers, auto-commit triggers
- **Modify:** `crates/darkly/src/config.rs` — transform tool hotkey, confirm/cancel hotkeys

---

## Phase 8: Warp Transforms

**Goal:** Mesh-based and displacement-based warping.

### Mesh Warp (Perspective/Envelope)

- Subdivide the source texture's bounding box into an NxN grid of quads
- Each quad vertex has a position and UV
- User drags control points → vertex positions change → mesh deforms
- GPU renders the mesh with the source texture mapped via UVs
- Same commit path as Phase 7

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

Phase 6 (Copy/Paste) ──→ Phase 7 (floating paste-in-place)

Phase 7 (Floating Content + Transforms) ──→ Phase 8 (Warp)
```

Phases 1 and 2 can be built in parallel. After those, phases 3–6 are independently buildable. Phases 7–8 are sequential.

---

## Verification

After each phase, verify:

1. **Phase 1:** Unit test — create `AlphaMask`, fill rect, boolean add/subtract, verify values. Ensure existing `TileGrid` (now `TileStore<Rgba>`) compiles and passes all existing tests.
2. **Phase 2:** Visual — use `overlay_debug` POC tool to render primitives (line, circle, rect) on canvas. Verify inverted color contrast on light/dark backgrounds. Verify no performance regression (should be instant, not laggy). Verify no CPU spike when primitives are static.
3. **Phase 3:** Visual — draw rectangle selection, paint across boundary, verify clipping. Verify marching ants animate. Test boolean modifiers.
4. **Phase 4:** Visual — each selection tool produces correct mask shape.
5. **Phase 5:** Visual — add mask to layer, paint on mask, verify transparency. Convert mask↔selection.
6. **Phase 6:** Functional — copy selection, paste, verify new layer content matches.
7. **Phase 7:** Visual + Functional — paste-in-place shows floating preview at original position; transform handles work (move, scale, rotate); Enter commits to tiles with correct blending; Escape cancels cleanly; cross-paste between masks and layers converts correctly; undo restores pre-commit state; auto-commit triggers on layer switch/tool change.
8. **Phase 8:** Visual — drag warp grid, verify mesh deformation. Liquify brush displaces pixels.

Build verification: `cargo build --target wasm32-unknown-unknown` after each phase. Run `wasm-pack build` and test in browser.
