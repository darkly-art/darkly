# GPU Performance Lessons Learned

## 1. Selection marching ants: primitive count explosion

**Problem**: Marching squares contour extraction produced one `OverlayPrimitive` per boundary pixel. A 200×200 rectangle selection = ~800 GPU instances, each with its own bounding quad and SDF evaluation. This caused ~4× GPU overhead compared to the overlay_debug tool (which uses ~5-10 primitives). Compounded by using `FLAG_INVERT_COLOR`, which triggers a full-resolution `copy_texture_to_texture` every frame so the shader can sample the background.

**Root cause**: Treating the overlay system (designed for a handful of transient tool-feedback primitives) as a general-purpose vector renderer for hundreds of persistent contour segments.

**Fix (visual)**: Scrapped `FLAG_INVERT_COLOR` for marching ants entirely. Used black and white solid-color dashed lines instead (like Krita) — no background sampling, no `copy_texture_to_texture`. Note: tool previews during drag can still use `FLAG_INVERT_COLOR` safely since they're transient (1 primitive, cleared on pointer up).

**Fix (primitive count)**: Two-stage reduction in `mask.rs`:
1. `merge_collinear()` — merges exactly horizontal/vertical segments. Handles rectangles: ~800 → ~4.
2. `simplify_segments()` — chains remaining diagonal segments into polylines via endpoint adjacency, then applies Ramer-Douglas-Peucker (epsilon=1px). Handles curves (ellipses, polygons): ~400 → ~20-30.

Both stages run only when the selection changes (not per-frame). Stage 2 is skipped when ≤32 segments (fast path for rectangles after stage 1).

## 2. Overlay render pass overhead for persistent primitives

**Problem**: Even after fixing the primitive count (rect selection = ~8 primitives), having a selection active adds ~30-40% GPU overhead when a veil is already driving 60fps rendering. The overlay ran as a separate render pass with `LoadOp::Load` — the GPU reads the entire framebuffer back from VRAM into tile memory just to draw 8 tiny quads on top. It also maintained a viewport-sized snapshot texture (unused since we dropped invert mode) and recreated a `wgpu::BindGroup` every frame.

**Root cause**: Independent animation throttles triggering extra frame renders. The overlay's `update_time()` set `needs_present = true` at ~10fps. Veils animate at 24fps via their own `anim_accum` throttle. These are independent timers — overlay ticks landed on frames where the veil throttle would have returned early, causing the compositor to run a full present+veil render on what should have been an idle frame. The overlay wasn't expensive to draw; it was forcing the veil to render extra frames.

**Key debugging insight**: overlay_debug uses the same overlay system with similar primitive counts but adds zero overhead. The difference: overlay_debug has no `needs_animation()` (no dashed lines), so it never sets `needs_present`. The overlay system, pipeline, shaders, and render pass were all innocent — binary elimination (skip draw call → still slow, skip animation tick → fixed) isolated the cause in two tests.

**Fix attempt 1 — eliminate separate render pass**: Split `encode()` into `prepare()` + `draw_solid()` + `encode_invert()`. Solid overlay primitives now draw at the end of the final present or veil-blit render pass. Added a 1×1 dummy texture so the solid-only path avoids allocating a viewport-sized snapshot. Minor improvement but not the root cause.

**Fix — unified frame scheduler**: Replaced independent per-system animation throttles with a master frame clock (`frame_count` in compositor). Systems register at fractional rates of the rAF master clock via integer divisors: veils at divisor 2 (50% = 30fps), overlay at divisor 4 (25% = 15fps). Divisors guarantee alignment — a divisor-4 tick always coincides with a divisor-2 tick, so systems never force extra renders. No system sets `needs_present` independently; the compositor's scheduler decides when to render. Config keys: `animation.veil_divisor`, `animation.overlay_divisor`.

## 3. Magic wand / flood fill: per-pixel HashMap lookups on sparse tile grid

**Problem**: Magic wand selection on a simple shape took ~2000ms. Even after switching from pixel-by-pixel BFS (with `HashSet<(i32, i32)>` visited tracking) to a scanline segment queue, it was still orders of magnitude slower than GIMP.

**Root cause**: Every pixel access went through `TileStore::get()` — a `HashMap<(i32, i32)>::get()` call. The scanline structure was correct but the helpers (`read_pixel`, `mask.sample`, `fill_span`) each did a hash lookup per pixel. For a 1920×1080 full-canvas fill: `scan_row` iterates ~1920 pixels per row × ~1080 rows × 2 lookups (source + mask) = ~4M hash lookups. Plus `fill_span` doing `get_or_create` per pixel = another ~2M. Total ~6-8M HashMap operations.

**Prior art (Krita)**: `KisScanlineFill` uses `numContiguousColumns()` to batch pixel reads — one tile accessor call per tile boundary, then direct pointer arithmetic within the tile. Pixels within the same 64×64 tile are contiguous in memory.

**Prior art (GIMP)**: `find_contiguous_segment()` uses `GeglSampler` which caches the current tile, giving O(1) sequential access within a tile.

**Fix — tile-aware scanning**: Restructured all flood fill helpers to work tile-by-tile:
- `find_segment()`: looks up one source tile per 64px boundary, scans pixels via direct array indexing on `RgbaData`. Empty tiles (no HashMap entry) are checked once and the entire 64px chunk is skipped or filled.
- `fill_span()`: one `get_or_create()` per tile, writes pixels directly to `AlphaF32Data`.
- `scan_row()`: looks up both source and mask tiles once per tile column, iterates pixels with array indexing — no `mask.sample()` or `read_pixel()` calls.

This reduced HashMap lookups from ~8M to ~1K for a full-canvas fill (30×17 = 510 tiles × ~2 lookups each).

**Takeaway**: Sparse tile grids are great for memory efficiency but toxic for per-pixel iteration. Any algorithm that touches every pixel in a region must batch access at the tile level. This applies to any future pixel-scanning code (color picker sampling, histogram computation, etc.).

## 4. CPU/GPU pixel-center convention mismatch in transform rasterization

**Problem**: Every fractional translation during a transform cycle shaved ~1px off the top and left edges of the content. Repeated transform-commit cycles caused progressive content loss.

**Root cause**: The CPU rasterization loop in `rasterize_to_tiles` sampled at integer pixel positions `(px, py)`, while the GPU fragment shader samples at pixel centers `(px + 0.5, py + 0.5)`. This is a fundamental GPU convention (defined in D3D, OpenGL, Vulkan, WebGPU specs): a pixel at integer coords `(i, j)` occupies the area `[i, i+1) × [j, j+1)` with its center at `(i + 0.5, j + 0.5)`. The hardware rasterizer evaluates all fragment positions at these half-integer centers.

For fractional translations, this caused the CPU bounds check to reject pixels the GPU correctly accepts. Example with translation `(89.416, 46.593)` from origin `(555, 328)`:
- CPU at pixel 644: `src_x = 89 - 89.416 = -0.416` → rejected (< 0)
- GPU at pixel 644: fragment center at 644.5, `src_x = 89.5 - 89.416 = 0.084` → accepted

Similarly, the GPU hardware bilinear sampler uses `u × N − 0.5` to convert UV to texel index — the same half-pixel shift appears in texture sampling.

**Fix**: Two changes to match the GPU convention:
1. `rasterize_to_tiles`/`rasterize_to_mask`: sample at pixel centers — `local_x = px + 0.5 - origin_x`
2. `sample_bilinear`: convert from pixel-center to texel-index space via `sx - 0.5`, with bounds check adjusted to `[-0.5, w-0.5]` to allow the half-texel clamp-to-edge border.

**Takeaway**: Any CPU code that replicates what a GPU shader does must use pixel-center coordinates `(i + 0.5, j + 0.5)`, not integer positions `(i, j)`. The 0.5 offset is not a fudge factor — it's a spec-defined convention. This applies to any future CPU-side rasterization, ray casting, or texture sampling that needs to match GPU output.
