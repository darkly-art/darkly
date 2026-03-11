# Phase 5: Layer Masks — Implementation Plan

## Context

Phase 5 adds non-destructive per-layer alpha masks (Photoshop-style). Each raster layer gets an optional `AlphaMask` that modulates its alpha during GPU compositing. Masks are paintable (white=reveal, black=hide), toggleable, and convertible to/from selections. The `AlphaMask` infrastructure from Phase 1 is reused — both selections and masks are `TileStore<AlphaF32>`.

## Prior Art

### Krita (`krita/libs/image/`)
- **Data model:** `KisTransparencyMask` → `KisMask` → `KisNode`. Mask holds a `KisSelection` containing a `KisPixelSelection` (8-bit paint device). Default pixel = **WHITE (255)** = reveal all.
- **Compositing:** `KisLayer::applyMasks()` calls `mask->apply()` which calls `mergeInMaskInternal()` — multiplies layer alpha by mask alpha per-pixel during the layer's projection update.
- **Paint routing:** Mask inherits `KisIndirectPaintingSupport`. Strokes go to a `temporaryTarget()` buffer, then merged into the mask selection on stroke end. This supports non-blocking reads during painting.
- **Dirty tracking:** Unified — mask dirty triggers `notifyChildMaskChanged()` on the parent layer, which marks the layer projection as needing update. No separate mask dirty system.
- **Group masks:** Groups become "isolated roots" when they have masks, forcing self-contained composition into a temp buffer before mask application.

### GIMP (`gimp/app/core/`)
- **Data model:** `GimpLayerMask` extends `GimpChannel` (which extends `GimpDrawable`). Mask is just a grayscale drawable with a back-pointer to its layer. Always single-channel format.
- **Three independent flags:** `edit_mask` (paint target), `apply_mask` (compositor uses it), `show_mask` (display mask as grayscale instead of layer). These are independent states.
- **Compositing:** GEGL node graph — layer connects to mode node's main input, mask connects to `aux2` input. The mode op multiplies layer alpha by mask value.
- **Paint routing:** When `edit_mask == TRUE`, the image's active drawable list returns the mask instead of the layer. No special paint code — mask is just another drawable. This is the cleanest design.
- **Dirty optimization:** Mask updates only invalidate the layer composite when `apply_mask || show_mask`. Dormant masks (exists but disabled) don't trigger recomposite.
- **Apply mask (destructive):** `gimp_layer_apply_mask()` permanently bakes mask into layer alpha, then removes it. Requires layer to have an alpha channel.
- **Group masks:** Mask resized dynamically via `gimp_group_layer_update_mask_size()` when group bounds change. New areas initialized to black (hidden).

### Key takeaways for our design
1. **Default = white (1.0)** — both editors agree
2. **Paint routing via flag, not separate API** — GIMP's `edit_mask` flag is the cleanest approach. The existing stroke API (`begin_stroke/stroke_to/end_stroke`) checks the flag and routes internally. No separate `begin_mask_stroke` methods needed.
3. **`show_mask` flag** — essential for UX. Users need to see the mask as grayscale to paint it precisely. The shader needs a `show_mask` uniform.
4. **Dirty gating** — mask edits should only trigger recomposite when mask is applied or shown (GIMP's optimization)
5. **`apply_mask()` destructive op** — standard in both editors. Bake mask into layer alpha permanently.

---

## Architecture

**GPU approach:** A second bind group (group 1) holds the mask texture (R8Unorm). The composite shader samples it and multiplies `fg.a * mask_alpha` when `apply_mask` is active. When `show_mask` is active, the shader outputs the mask value as grayscale instead of the layer content. Layers without masks bind a 1x1 white fallback texture → no effect.

**Mask default:** New masks start fully white (1.0 = reveal all), matching both Krita and GIMP. A `Tile::full()` LazyLock singleton (all 1.0) with COW sharing keeps memory at ~100KB even for large canvases. `get_or_create_full()` ensures new tiles during painting default to 1.0, not 0.0.

**Paint routing (GIMP model):** Per-layer `editing_mask: bool` flag. When true, the existing `begin_stroke/stroke_to/end_stroke` API routes paint ops to the mask's `MaskPaintTarget` instead of the layer's `PaintTarget`. No separate mask stroke methods. The mask is just another paint surface.

**Three flags per layer (GIMP model):**
- `mask_enabled: bool` — whether mask modulates alpha during compositing (GIMP's `apply_mask`)
- `show_mask: bool` — whether to display the mask as grayscale instead of layer content
- `editing_mask: bool` — whether painting targets the mask (runtime state, not persisted)

**Group masks:** Data structures added to `LayerGroup` but GPU compositing deferred — requires isolated group rendering (non-passthrough buffers), same as both Krita and GIMP require.

---

## Steps (dependency-ordered)

### Step 1: Tile System — `Tile::full()` + `get_or_create_full()`
**File: `crates/darkly/src/tile.rs`**

- Add `Tile<AlphaF32>::full()` — LazyLock shared Arc filled with `1.0f32` (mirrors `Tile::empty()`)
- Add `impl TileStore<AlphaF32> { fn get_or_create_full() }` — same as `get_or_create()` but uses `Tile::full` as default, hooks into recording/memento system

### Step 2: Layer Structs — Add Mask Fields
**File: `crates/darkly/src/layer.rs`**

- Add to `RasterLayer`:
  - `pub mask: Option<AlphaMask>` — the mask data
  - `pub mask_enabled: bool` — whether mask affects compositing (GIMP's `apply_mask`)
  - `pub show_mask: bool` — display mask as grayscale instead of layer content
- Add to `LayerGroup`: same three fields
- Update constructors: `mask: None, mask_enabled: true, show_mask: false`

### Step 3: Mask Paint Target
**File: `crates/darkly/src/paint.rs`**

- Add `MaskPaintTarget { mask: &mut AlphaMask, dirty: &mut DirtyRegion }`
- `paint(px, py, value, strength)` — blends toward value using `get_or_create_full()`
- `erase(px, py, strength)` — blends toward 0.0
- Following GIMP: mask is just another paint surface, no special logic

### Step 4: Document — Mask Operations
**File: `crates/darkly/src/document.rs`**

- Add field: `pub mask_dirty: HashMap<LayerId, DirtyRegion>`
- Add `pub(crate) fn find_mask_fields_mut()` — tree traversal helper
- Add mask CRUD: `add_mask()`, `remove_mask()`, `set_mask_enabled()`, `set_show_mask()`
- Add mask paint helpers: `make_mask_paint_target()`, `paint_mask_circle()`, `erase_mask_circle()`
- Add mask transactions: `begin_mask_transaction()`, `commit_mask_transaction()`
- Add conversion: `selection_to_mask()`, `mask_to_selection()`
- Add destructive: `apply_mask()` — bake mask into layer alpha (multiply each pixel), remove mask (following GIMP)

### Step 5: Undo — MaskTileAction + MaskPropertyAction
**File: `crates/darkly/src/undo/mask.rs`** (new)
**File: `crates/darkly/src/undo/mod.rs`** (add module)

- `MaskTileAction(LayerId, Memento<AlphaF32>)` — flip-flop rollback, populates `doc.mask_dirty`, returns empty HashMap
- `MaskPropertyAction(LayerId, Option<AlphaMask>, bool, bool)` — swaps mask + mask_enabled + show_mask state

### Step 6: Composite Shader — Mask Sampling + Show Mask
**File: `shaders/composite.wgsl`**

- Add `@group(1) @binding(0) var t_mask: texture_2d<f32>;`
- Repurpose `_pad0` in `Uniforms` → `show_mask: u32`
- In `fs_main`:
  ```wgsl
  let mask_alpha = textureSample(t_mask, t_sampler, in.uv).r;
  if (uniforms.show_mask != 0u) {
      // Display mask as grayscale (GIMP's show_mask mode)
      return vec4f(mask_alpha, mask_alpha, mask_alpha, 1.0);
  }
  fg = vec4f(fg.rgb, fg.a * uniforms.opacity * mask_alpha);
  ```
- When mask not applied: 1x1 white fallback → mask_alpha=1.0 → no effect
- When show_mask active: renders the mask itself as grayscale image

### Step 7: GPU Pipeline — Mask Bind Group Layout
**File: `crates/darkly/src/gpu/blend.rs`**

- Add `mask_bind_group_layout` to `BlendPipelines` (1 entry: `texture_2d<f32>`)
- Update pipeline layout: `bind_group_layouts: &[&blend_bgl, &mask_bgl]`
- Rename `_pad0` → `show_mask` in `BlendUniforms`

**File: `crates/darkly/src/gpu/atlas.rs`**

- Add `LayerTexture::with_format(device, w, h, format)` — extract core logic
- Add `LayerTexture::new_mask(device, w, h)` — R8Unorm variant

### Step 8: Compositor — Mask Textures + Upload + Dirty Gating
**File: `crates/darkly/src/gpu/compositor.rs`**

- Add fields: `mask_textures: HashMap<LayerId, LayerTexture>`, `default_mask_view`, `default_mask_bind_group`
- Add `mask_bind_group` to `RasterLayerCache`
- In `Compositor::new()`: create 1x1 R8Unorm white texture + default bind group
- In `ensure_raster_layer()`: init mask_bind_group with default
- Add `set_layer_mask(device, queue, layer_id, has_mask)` — creates/removes R8Unorm texture, rebuilds mask bind group
- Add `update_mask_binding(device, layer_id, mask_enabled, show_mask)` — swaps real vs default mask bind group. Following GIMP's dirty optimization: bind real mask only when `mask_enabled || show_mask`
- In `render_offscreen()`: upload dirty mask tiles (f32→u8 conversion, `bytes_per_row = TILE_SIZE`). Only upload when `mask_enabled || show_mask` (GIMP's dormant mask optimization).
- In render pass: add `rpass.set_bind_group(1, &cache.mask_bind_group, &[])`
- Add `update_raster_uniforms` to include `show_mask` flag

### Step 9: Engine — Mask API + Paint Routing
**File: `crates/darkly/src/engine.rs`**

Following GIMP's `edit_mask` flag approach — reuse existing stroke API:

- Add per-layer state tracking: `editing_mask_layer: Option<u64>` — which layer has mask editing active
- **Modify existing `begin_stroke()`**: if `editing_mask_layer == Some(layer_id)`, call `doc.begin_mask_transaction()` instead of `doc.begin_transaction()`
- **Modify existing `stroke_to()`**: if editing mask, route paint ops to `doc.paint_mask_circle()` / `doc.erase_mask_circle()` instead of layer paint methods
- **Modify existing `end_stroke()`**: if editing mask, commit mask transaction and push `MaskTileAction` instead of `TileAction`
- Add mask API:
  - `add_mask(layer_id)` — creates mask, pushes MaskPropertyAction
  - `remove_mask(layer_id)` — removes mask, pushes MaskPropertyAction
  - `apply_mask(layer_id)` — bake mask into layer alpha (destructive), pushes TileAction + MaskPropertyAction
  - `set_mask_enabled(layer_id, enabled)` — toggles compositor binding
  - `set_show_mask(layer_id, show)` — toggles grayscale display mode
  - `set_editing_mask(layer_id, editing)` — sets paint target (runtime, not undoable)
  - `selection_to_mask(layer_id)`, `mask_to_selection(layer_id)` — conversions
- Update `LayerInfo::Raster`: add `has_mask: bool, mask_enabled: bool, show_mask: bool`
- Update `node_to_layer_info()`, `sync_compositor_layers()`

### Step 10: WASM Bridge
**File: `frontend/wasm/src/api.rs`**

- Add passthrough methods: `add_mask`, `remove_mask`, `apply_mask`, `set_mask_enabled`, `set_show_mask`, `set_editing_mask`, `selection_to_mask`, `mask_to_selection`
- No separate mask stroke methods — existing `begin_stroke/stroke_to/end_stroke` handle mask painting via the `editing_mask` flag (GIMP model)

---

## Key Design Details

| Decision | Prior Art | Rationale |
|----------|-----------|-----------|
| Default mask = white (1.0) | Both Krita and GIMP | Universally expected. "Add mask" reveals all by default. |
| Paint routing via `editing_mask` flag | GIMP's `edit_mask` | Reuses existing stroke API. Mask is just another drawable. No API duplication. |
| Three independent flags | GIMP's `apply_mask`/`show_mask`/`edit_mask` | Orthogonal controls. Show mask while it's disabled. Edit without applying. |
| `show_mask` renders grayscale | Both editors | Essential for mask editing UX. Uses `_pad0` uniform slot. |
| Dirty gating on `mask_enabled \|\| show_mask` | GIMP's optimization | Dormant masks don't trigger recomposite or upload. |
| `apply_mask()` destructive op | GIMP's `gimp_layer_apply_mask()` | Standard workflow: bake mask into alpha permanently. |
| Separate bind group (group 1) | N/A (GPU-specific) | Avoids rebuilding existing group 0 bind groups on mask change. |
| R8Unorm (not R32Float) | GIMP uses 8-bit masks | 4x less GPU memory, no `float32-filterable` feature needed. |
| `Tile::full()` with LazyLock COW | Krita's white default pixel | COW sharing: all initial tiles share one 16KB Arc. |
| `mask_dirty` separate from `dirty` | N/A (tile-GPU-specific) | Mask tile uploads are independent of layer tile uploads. |
| Group masks data-only | Both require isolated compositing | GPU group isolation not yet implemented. Structs are ready. |

## Verification

1. `cargo build --target wasm32-unknown-unknown` — must compile
2. Add mask to a layer → layer still fully visible (white mask)
3. Paint black on mask → masked areas become transparent
4. Toggle `mask_enabled` off → layer fully visible (mask dormant, no recomposite cost)
5. Toggle `show_mask` on → see mask as grayscale image
6. Set `editing_mask` → paint strokes target mask instead of layer
7. Convert selection → mask → selection roundtrip preserves shape
8. `apply_mask` bakes mask into layer alpha and removes mask
9. Undo/redo mask painting restores previous mask state
10. Undo add_mask removes the mask entirely
11. Performance: no regression on layers without masks (1x1 white texture, no upload)
