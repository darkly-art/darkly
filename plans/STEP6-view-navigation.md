# Phase 2, Session 2 — View Transform + Canvas Navigation

## Scope

Steps 3–4 from the Phase 2 plan. Add a view transform to the present pipeline (Rust + shader) so the canvas can be panned, zoomed, and rotated without affecting compositing. Then build the canvas navigation state machine in the frontend.

## Prerequisites

Session 1 complete: config system with presets and reactive stores, AppState singleton with view transform fields (`panX`, `panY`, `zoom`, `rotation`).

## Done When

- `set_view_transform()` pans/zooms/rotates the canvas correctly
- View-only changes skip compositing and only re-present (performance)
- Space+drag pans, Shift+Space+drag rotates, Ctrl+Space+drag zooms
- Ctrl+scroll zooms (cursor-centered)
- Canvas fits the viewport on load (fit-to-view)
- Painting works correctly after transform (mouse coords inverse-transformed)
- Navigation keys read from config (changing preset changes bindings)

---

## Step 3: Canvas view transform (Rust + shader)

### `crates/darkly/src/gpu/view.rs`

```rust
/// 2D view transform for canvas navigation.
/// Compositing happens in canvas-pixel space. This transform is applied
/// only in the present shader to map canvas pixels to screen pixels.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewTransform {
    /// Inverse view matrix (screen -> canvas), stored as 3 vec4s for std140.
    /// Row 0: [m00, m01, canvas_w, 0]
    /// Row 1: [m10, m11, canvas_h, 0]
    /// Row 2: [tx,  ty,  1,        0]
    pub matrix: [[f32; 4]; 3],
}

impl ViewTransform {
    pub fn identity() -> Self;

    /// Build the inverse view matrix from pan/zoom/rotation.
    /// Forward: canvas -> screen (translate -center, scale, rotate, translate +screen_center+pan)
    /// Shader needs inverse: screen -> canvas.
    pub fn from_pan_zoom_rotate(
        pan_x: f32, pan_y: f32,
        zoom: f32, rotation: f32,
        screen_w: f32, screen_h: f32,
        canvas_w: f32, canvas_h: f32,
    ) -> Self;

    /// Transform screen point to canvas coordinates using the stored inverse matrix.
    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> (f32, f32);
}
```

Key math for `from_pan_zoom_rotate`:
```
inv_zoom = 1.0 / zoom
m00 = cos(rotation) * inv_zoom
m01 = sin(rotation) * inv_zoom
m10 = -sin(rotation) * inv_zoom
m11 = cos(rotation) * inv_zoom
tx = cx - m00 * sx - m10 * sy    (cx = canvas_w/2, sx = screen_w/2 + pan_x)
ty = cy - m01 * sx - m11 * sy    (cy = canvas_h/2, sy = screen_h/2 + pan_y)
```

Canvas dimensions packed into the matrix (row0.z = canvas_w, row1.z = canvas_h) for the present shader's OOB check.

### Compositor changes (`compositor.rs`)

- Add `view_uniform_buf: wgpu::Buffer` created once in `Compositor::new()` (P1 compliance)
- Present bind group layout gains binding 2 (uniform buffer)
- Add `pub fn update_view_transform(&self, queue, transform)` — calls `queue.write_buffer()`
- Add `needs_present: bool` flag separate from `needs_composite`
- `update_view_transform` sets `needs_present = true` without setting `needs_composite`

**View-only optimization:** When `!needs_composite && needs_present`, skip all tile upload and compositing, only run the present pass from the existing `composite_cache`. This means panning/zooming/rotating is essentially free — just a uniform buffer write and one render pass.

### Updated `shaders/present.wgsl`

**Tile-padded textures vs canvas dimensions:** Layer textures are padded to tile boundaries (e.g. 900px -> 960px at TILE_SIZE=64). The present shader must use padded texture size for UV sampling (1:1 texel mapping) but actual canvas dimensions for OOB check (so padding area shows workspace background, not black).

```wgsl
struct ViewTransform {
    row0: vec4f,
    row1: vec4f,
    row2: vec4f,
}

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var t_sampler: sampler;
@group(0) @binding(2) var<uniform> view: ViewTransform;

@fragment fn fs_present(in: VertexOutput) -> @location(0) vec4f {
    let screen_pos = in.position.xy;
    let canvas_x = view.row0.x * screen_pos.x + view.row1.x * screen_pos.y + view.row2.x;
    let canvas_y = view.row0.y * screen_pos.x + view.row1.y * screen_pos.y + view.row2.y;

    let tex_dims = vec2f(textureDimensions(t_source));
    let uv = vec2f(canvas_x, canvas_y) / tex_dims;
    let clamped_uv = clamp(uv, vec2f(0.0), vec2f(1.0));
    let color = textureSample(t_source, t_sampler, clamped_uv);

    let canvas_dims = vec2f(view.row0.z, view.row1.z);
    let oob = canvas_x < 0.0 || canvas_x > canvas_dims.x
           || canvas_y < 0.0 || canvas_y > canvas_dims.y;
    let bg = vec4f(0.11, 0.11, 0.11, 1.0);
    return select(vec4f(color.rgb, 1.0), bg, oob);
}
```

### WASM bridge additions (`api.rs`)

**Document size is decoupled from viewport:** `DarklyHandle::create` accepts explicit `doc_width`/`doc_height` for the document dimensions. Viewport size comes from the HTML canvas element.

```rust
pub async fn create(canvas: HtmlCanvasElement, doc_width: u32, doc_height: u32) -> DarklyHandle;

pub fn set_view_transform(
    &mut self, pan_x: f32, pan_y: f32,
    zoom: f32, rotation: f32,
    screen_w: f32, screen_h: f32,
);

pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32>;
```

---

## Step 4: Canvas navigation state machine

### `frontend/src/canvas/navigation.svelte.ts`

Key design decisions:

- **Config-driven modifiers.** Reads `hotkeys.nav.trigger` (default `'Space'`), `hotkeys.nav.zoom` (default `'Ctrl'`), `hotkeys.nav.rotate` (default `'Shift'`). A `hasModifier(e, mod)` helper maps config string to `e.ctrlKey`/`e.shiftKey`/`e.altKey`.
- **Rotation is Krita-style angular.** On pointer down, measure angle from canvas center to cursor with `atan2()`. On move, compute delta angle. Feels like physically spinning the canvas.
- **Rotation pivot accounts for pan.** Canvas center on screen = `element_center + pan`. Angular measurement uses this point, not raw element center.
- **Zoom uses vertical drag.** Drag up = zoom out, drag down = zoom in. Exponential: `Math.pow(2, -dy / 150)`.
- **Pan is 1:1 CSS pixel mapping.**
- **Scroll zoom is cursor-centered:** Adjusts pan so the point under the cursor stays fixed after zoom.
- **Mode-aware cursor.** Reactive `cursor` getter returns: `grab` (space held), `grabbing` (panning), `zoom-in` (zooming), custom SVG for rotation. Default `crosshair`.

### `frontend/src/canvas/CanvasView.svelte`

Moves the current `App.svelte` canvas+mouse logic here. Pointer event flow:

1. Navigation state machine gets first chance (`nav.onPointerDown(e)`)
2. If navigation consumed the event, skip tool dispatch
3. Otherwise, `setPointerCapture(e.pointerId)` for continuous strokes
4. Transform screen coords -> canvas coords via `handle.screen_to_canvas()`
5. Dispatch to active tool

**Initial fit-to-view:** After init, compute zoom that fits document in viewport without scaling up:
```typescript
Math.min(canvas.width / DOC_WIDTH, canvas.height / DOC_HEIGHT, 1)
```

**View transform sync via `$effect`:**
```typescript
$effect(() => {
    if (app.handle && canvas) {
        const dpr = window.devicePixelRatio || 1;
        app.handle.set_view_transform(
            app.panX * dpr, app.panY * dpr,
            app.zoom, app.rotation,
            canvas.width, canvas.height,
        );
    }
});
```

**Canvas buffer sizing:** `ResizeObserver` keeps the buffer and GPU surface in sync with the CSS layout at `devicePixelRatio`. `cssToBuffer` scales by DPR (no letterbox math — buffer matches element 1:1).

### Verification

- Canvas fits viewport on load (scaled down for large documents, never up)
- Space+drag pans
- Shift+Space+drag rotates (angular, Krita-style)
- Ctrl+Space+drag zooms (vertical, exponential)
- Ctrl+scroll zooms (cursor-centered)
- All navigation keys respect config
- Painting works correctly after transform
- View-only changes skip compositing (P2 optimization)
- Cursor changes appropriately during navigation

---

## Files Created/Modified This Session

```
crates/darkly/src/gpu/
├── view.rs                    # NEW: ViewTransform struct
├── compositor.rs              # MODIFIED: view uniform, needs_present flag
└── mod.rs                     # MODIFIED: pub mod view

shaders/
└── present.wgsl               # MODIFIED: view transform matrix

frontend/
├── src/
│   ├── canvas/
│   │   ├── CanvasView.svelte  # NEW: extracted from App.svelte
│   │   └── navigation.svelte.ts # NEW: pan/zoom/rotate state machine
│   └── App.svelte             # MODIFIED: delegates to CanvasView
└── wasm/src/
    └── api.rs                 # MODIFIED: set_view_transform, screen_to_canvas, create(doc_w, doc_h)
```
