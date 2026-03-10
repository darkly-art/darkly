# GPU Performance Lessons Learned

## 1. Selection marching ants: primitive count explosion

**Problem**: Marching squares contour extraction produced one `OverlayPrimitive` per boundary pixel. A 200×200 rectangle selection = ~800 GPU instances, each with its own bounding quad and SDF evaluation. This caused ~4× GPU overhead compared to the overlay_debug tool (which uses ~5-10 primitives). Compounded by using `FLAG_INVERT_COLOR`, which triggers a full-resolution `copy_texture_to_texture` every frame so the shader can sample the background.

**Root cause**: Treating the overlay system (designed for a handful of transient tool-feedback primitives) as a general-purpose vector renderer for hundreds of persistent contour segments.

**Fix**: Scrapped the invert overlay approach entirely. Used black and white marching ants instead (like Krita) — two passes of solid-color dashed lines, no background sampling, no texture copy. Also merged collinear contour segments to reduce primitive count.

## 2. Overlay render pass overhead for persistent primitives

**Problem**: Even after fixing the primitive count (rect selection = ~8 primitives), having a selection active adds ~30-40% GPU overhead when a veil is already driving 60fps rendering. The overlay ran as a separate render pass with `LoadOp::Load` — the GPU reads the entire framebuffer back from VRAM into tile memory just to draw 8 tiny quads on top. It also maintained a viewport-sized snapshot texture (unused since we dropped invert mode) and recreated a `wgpu::BindGroup` every frame.

**Root cause**: The overlay render pass was architecturally separate from the present/veil-blit pass. On tile-based GPUs, the load+store cycle for the entire framebuffer is expensive relative to the trivial overlay geometry. A polyline approach (Krita/GIMP style) wouldn't help — the overhead was from the render pass, not the primitive count.

**Fix attempt 1 — eliminate separate render pass**: Split `encode()` into `prepare()` (CPU-side uploads) + `draw_solid()` (draw calls only). Solid overlay primitives now draw at the end of the final present or veil-blit render pass — no separate `LoadOp::Load` pass needed. Invert primitives (used by overlay_debug/rect_select) still get their own pass with snapshot copy. Also added a 1×1 dummy texture so the solid-only path avoids allocating a viewport-sized snapshot.

**Result**: Minor improvement, but ~30% overhead persists. The separate render pass / LoadOp::Load was not the primary cost. The overhead source is still unidentified — it's not primitive count, not the render pass, not the snapshot texture, not bind group creation. Needs further profiling to isolate.
