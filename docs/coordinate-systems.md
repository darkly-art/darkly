# Coordinate Systems

Darkly moves a pixel through several frames. Most code only ever sees two or
three of them, but a value carried into the wrong frame is the single most
recurring class of bug here — and it stays *invisible* until the canvas is
cropped, because `canvas_origin == (0, 0)` makes two of the frames coincide.

Top to bottom, with their authority:

- **Screen / CSS** (`clientX/Y`) → **Buffer / device px** (`× devicePixelRatio`).
  The DOM boundary. Owned *solely* by [`frontend/src/canvas/coordinates.ts`](../frontend/src/canvas/coordinates.ts);
  nothing else multiplies by DPR or reads `getBoundingClientRect`.
- **Plane** — `coord::CanvasPoint` / `CanvasRect`. The absolute document frame;
  **it does not move when you crop**. Layers, paint, the brush cursor/hover, undo
  regions, and the overlay's `plane_fwd` matrix all live here. May be negative
  (paste-extent layers sit at negative plane offsets).
- **Window-local** — `coord::WindowPoint` / `WindowRect`. Origin at the
  canvas-window top-left, i.e. plane `canvas_origin`. The **selection texture**
  and the **floating-preview texture** are window-sized and indexed here. The
  composite shader bridges the two with `window_uv_to_plane(uv, canvas_origin,
  canvas_size)`.
- **Layer-local** — `coord::LayerPoint` / `LayerRect`. A specific texture's own
  pixels, always non-negative. Converted only through its `LayerTexture`
  (`layer_to_canvas` / `canvas_to_layer_rect`).

`GpuPaintTarget` keeps two plane-space rectangles with different roles: the
target texture's extent locates its texels in the plane, while the canvas-window
rectangle anchors window-sized resources such as the selection texture. For an
extra-canvas layer or mask these rectangles differ; its node extent must never
be reused as the selection texture's sampling frame.

The invariant: **`plane = window_local + canvas_origin`**.

**How to convert — always through a named method, never hand-written `± canvas_origin`:**

- window-local ↔ plane: `WindowRect::to_canvas(origin)` / `CanvasRect::to_window(origin)`
  (and the `WindowPoint`/`CanvasPoint` equivalents).
- screen ↔ plane: `ViewTransform::screen_to_plane` (Rust) / `app.viewMatrices` (JS, via
  `coordinates.ts`).
- layer ↔ plane: `LayerTexture::layer_to_canvas` / `canvas_to_layer_rect`.

**Pitfalls (each has bitten us):**

- A window-local value stored in a plane-typed slot is the canonical bug. The
  selection texture / floating preview are window-local; older code stored their
  bounds in `CanvasRect`. `selection_pixel_bounds()` is now `WindowRect` for
  exactly this reason — lift it with `.to_canvas(doc.canvas_origin)` before it
  meets plane-space code.
- Overlay `FLAG_CANVAS_SPACE` primitives are **plane**. Window-local data
  (marching-ants contours) must be `.to_canvas(origin)` *before* it is pushed.
- When a texture is window-anchored, **every** canvas↔texel op on it must use the
  same anchor (`canvas_origin`). Mixing a `(0, 0)`-anchored frame with a
  `canvas_origin`-anchored frame inside one pass is what offset the transform
  preview and left ghost pixels behind.
- **Test with a non-zero `canvas_origin`** (crop first). A test at the default
  origin exercises none of this and will pass while the app is broken.
