# GPU Performance Lessons Learned

## 1. Selection marching ants: primitive count explosion

**Problem**: Marching squares contour extraction produced one `OverlayPrimitive` per boundary pixel. A 200×200 rectangle selection = ~800 GPU instances, each with its own bounding quad and SDF evaluation. This caused ~4× GPU overhead compared to the overlay_debug tool (which uses ~5-10 primitives). Compounded by using `FLAG_INVERT_COLOR`, which triggers a full-resolution `copy_texture_to_texture` every frame so the shader can sample the background.

**Root cause**: Treating the overlay system (designed for a handful of transient tool-feedback primitives) as a general-purpose vector renderer for hundreds of persistent contour segments.

**Fix**: Scrapped the invert overlay approach entirely. Used black and white marching ants instead (like Krita) — two passes of solid-color dashed lines, no background sampling, no texture copy. Also merged collinear contour segments to reduce primitive count.
