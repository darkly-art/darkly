# GPU Performance Lessons Learned

## 1. Selection marching ants: primitive count explosion

**Problem**: Marching squares contour extraction produced one `OverlayPrimitive` per boundary pixel. A 200×200 rectangle selection = ~800 GPU instances, each with its own bounding quad and SDF evaluation. This caused ~4× GPU overhead compared to the overlay_debug tool (which uses ~5-10 primitives). Compounded by using `FLAG_INVERT_COLOR`, which triggers a full-resolution `copy_texture_to_texture` every frame so the shader can sample the background.

**Root cause**: Treating the overlay system (designed for a handful of transient tool-feedback primitives) as a general-purpose vector renderer for hundreds of persistent contour segments.

**Fix**: Scrapped the invert overlay approach entirely. Used black and white marching ants instead (like Krita) — two passes of solid-color dashed lines, no background sampling, no texture copy. Also merged collinear contour segments to reduce primitive count.

## 2. Overlay render pass overhead for persistent primitives

**Problem**: Even after fixing the primitive count (rect selection = ~8 primitives), having a selection active adds ~30-40% GPU overhead when a veil is already driving 60fps rendering. The overlay ran as a separate render pass with `LoadOp::Load` — the GPU reads the entire framebuffer back from VRAM into tile memory just to draw 8 tiny quads on top. It also maintained a viewport-sized snapshot texture (unused since we dropped invert mode) and recreated a `wgpu::BindGroup` every frame.

**Root cause**: Independent animation throttles triggering extra frame renders. The overlay's `update_time()` set `needs_present = true` at ~10fps. Veils animate at 24fps via their own `anim_accum` throttle. These are independent timers — overlay ticks landed on frames where the veil throttle would have returned early, causing the compositor to run a full present+veil render on what should have been an idle frame. The overlay wasn't expensive to draw; it was forcing the veil to render extra frames.

**Key debugging insight**: overlay_debug uses the same overlay system with similar primitive counts but adds zero overhead. The difference: overlay_debug has no `needs_animation()` (no dashed lines), so it never sets `needs_present`. The overlay system, pipeline, shaders, and render pass were all innocent — binary elimination (skip draw call → still slow, skip animation tick → fixed) isolated the cause in two tests.

**Fix attempt 1 — eliminate separate render pass**: Split `encode()` into `prepare()` + `draw_solid()` + `encode_invert()`. Solid overlay primitives now draw at the end of the final present or veil-blit render pass. Added a 1×1 dummy texture so the solid-only path avoids allocating a viewport-sized snapshot. Minor improvement but not the root cause.

**Fix — unified frame scheduler**: Replaced independent per-system animation throttles with a master frame clock (`frame_count` in compositor). Systems register at fractional rates of the rAF master clock via integer divisors: veils at divisor 2 (50% = 30fps), overlay at divisor 4 (25% = 15fps). Divisors guarantee alignment — a divisor-4 tick always coincides with a divisor-2 tick, so systems never force extra renders. No system sets `needs_present` independently; the compositor's scheduler decides when to render. Config keys: `animation.veil_divisor`, `animation.overlay_divisor`.
