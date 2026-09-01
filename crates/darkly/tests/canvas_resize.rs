//! Canvas resize & crop-to-selection integration tests.
//!
//! Exercises the canvas-window coordinate model: a non-zero `canvas_origin`
//! (cropped) document, the selection-mask seams (the marquee must keep masking
//! the same *plane* pixels across a crop), and document-only resize undo.
//!
//! Run with: `cargo test -p darkly --test canvas_resize --features testing -- --test-threads=1`

use darkly::coord::{CanvasPoint, CanvasRect};
use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::overlay::{OverlayPrimitive, FLAG_CANVAS_SPACE, KIND_DASHED_LINE};
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn alpha_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[((y * w + x) * 4 + 3) as usize]
}

/// Paint a horizontal brush stroke at plane-y `py`, sweeping plane-x `[x0, x1)`.
fn paint_row(engine: &mut DarklyEngine, layer_id: LayerId, py: f32, x0: f32, x1: f32) {
    engine.begin_stroke(layer_id).unwrap();
    let steps = 48;
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * (i as f32 / steps as f32);
        engine.stroke_to(StrokeOp::BrushStroke {
            x,
            y: py,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
}

/// MARQUEE regression: a selection made before a crop must keep masking the
/// **same plane pixels** afterward. This exercises the selection-mask
/// re-realization (overlap copy preserving the plane anchor) and the brush
/// composite selection-UV seam (`(p - canvas_origin) / canvas_size`).
#[test]
fn marquee_selection_masks_same_plane_pixels_after_crop() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Select a vertical band: plane x in [8, 32), full height.
    engine.select_rect(8.0, 0.0, 24.0, h as f32, SelectionMode::Replace, false, 0.0);

    // Crop to a window anchored at plane (8, 0), size 40×64 — a NON-ZERO
    // origin. The selection band [8, 32) sits fully inside this window.
    engine.resize_canvas(CanvasRect::from_xywh(8, 0, 40, h));
    assert_eq!(engine.canvas_rect().origin, CanvasPoint::new(8, 0));

    // Paint across the window's plane-x at a plane-y inside the band's height.
    paint_row(&mut engine, layer_id, 32.0, 8.0, 48.0);

    // The raster layer is plane-anchored (origin (0,0), 64×64) and untouched by
    // the crop, so its readback is plane-indexed.
    let px = engine.test_readback_layer(layer_id);

    assert!(
        alpha_at(&px, w, 20, 32) > 0,
        "a selected plane pixel must still be painted through the marquee after crop"
    );
    assert_eq!(
        alpha_at(&px, w, 40, 32),
        0,
        "an unselected plane pixel (inside the new window) must stay transparent"
    );
}

/// Crop moves the canvas window and preserves off-window layer pixels — the
/// raster layer keeps its full plane extent; only display/export is clipped.
#[test]
fn crop_preserves_off_window_layer_pixels() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Paint a full-width row at plane y=10 (before any crop).
    paint_row(&mut engine, layer_id, 10.0, 0.0, w as f32 - 1.0);
    let before = engine.test_readback_layer(layer_id);
    let painted_x = (4..60)
        .find(|&x| alpha_at(&before, w, x, 10) > 0)
        .expect("row should have paint before crop");

    // Crop to a sub-window that EXCLUDES the painted row (window y starts at 20).
    engine.resize_canvas(CanvasRect::from_xywh(0, 20, 40, 30));

    // The painted pixel is now outside the window, but still on the layer.
    let after = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&after, w, painted_x, 10) > 0,
        "off-window layer pixels must be preserved on the layer across a crop"
    );
}

/// Resize/crop is document-only and exactly undoable: undo restores both the
/// origin and the dimensions.
#[test]
fn resize_canvas_undo_restores_window() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    engine.resize_canvas(CanvasRect::from_xywh(12, 8, 30, 40));
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(12, 8, 30, 40));

    engine.undo();
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(0, 0, w, h));

    engine.redo();
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(12, 8, 30, 40));
}

/// RECT-RESIZE regression: the interactive resize preview drives the canvas
/// window as an explicit plane-space rect (WASM `resize_canvas_rect`), which can
/// express a **pure translation** — same size, shifted origin. The retired
/// anchor-only path could not (a zero size-delta forced a zero offset). Moving
/// the window without resizing must land the origin exactly and undo cleanly.
#[test]
fn resize_canvas_pure_translation_moves_window_and_undoes() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    // Same dimensions, non-zero origin: a pure translation of the window.
    engine.resize_canvas(CanvasRect::from_xywh(10, 6, w, h));
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(10, 6, w, h));

    engine.undo();
    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(0, 0, w, h));
}

/// PRESENT-PATH regression: the cached view transform embeds the canvas
/// dimensions (`canvas_w/h` as the present shader's sampling-normalization +
/// the canvas center). A resize/crop changes the dims but is otherwise
/// document-only, so the view matrix must be **rebuilt** to match — otherwise
/// the present pass samples the new-size composite through a stale-dim matrix
/// and the image shows stretched/offset until the next pointer event re-pushes
/// the view (the reported "glitch that heals on interaction" / "stretched
/// content" bugs).
///
/// `screen_to_plane` reads the same cached `view_transform` the present pass
/// consumes, so probing it is the present-path invariant without GPU-readback
/// flakiness. With an identity-fit view (pan 0, zoom 1), the screen center
/// resolves to the canvas center `(canvas_w/2, canvas_h/2)` — which tracks the
/// matrix's embedded dims.
#[test]
fn resize_rebuilds_view_transform_for_new_dims() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    let (sw, sh) = (200.0_f32, 200.0_f32);
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, sw, sh);

    // Sanity: screen center maps to the original canvas center (32, 32).
    let (cx0, cy0) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx0 - 32.0).abs() < 1e-2 && (cy0 - 32.0).abs() < 1e-2,
        "pre-resize screen center should map to (32, 32), got ({cx0}, {cy0})"
    );

    // Grow the canvas WITHOUT any intervening set_view_transform.
    engine.resize_canvas(CanvasRect::from_xywh(0, 0, 128, 96));

    let (cx1, cy1) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx1 - 64.0).abs() < 1e-2,
        "screen center must map to the NEW canvas center x (64) after resize, got {cx1}"
    );
    assert!(
        (cy1 - 48.0).abs() < 1e-2,
        "screen center must map to the NEW canvas center y (48) after resize, got {cy1}"
    );

    // Undo reconciles dims back to 64×64 via the same chokepoint — the view
    // matrix must rebuild on undo too (bug #3: undo restores dims but shows
    // stretched).
    engine.undo();
    let (cx2, cy2) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (cx2 - 32.0).abs() < 1e-2 && (cy2 - 32.0).abs() < 1e-2,
        "after undo, screen center must map back to (32, 32), got ({cx2}, {cy2})"
    );
}

/// FRAME regression: the engine's screen→canvas query must return **plane**
/// coordinates (window-local + `canvas_origin`), not window-local. Tools and the
/// overlay consume this frame; returning window-local after a non-zero-origin
/// crop is exactly the paint-vs-hover-preview offset bug. With an identity-fit
/// view, the screen center resolves to `canvas_origin + window_center`.
#[test]
fn screen_to_plane_includes_canvas_origin() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    let (sw, sh) = (200.0_f32, 200.0_f32);
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, sw, sh);

    // Crop to a NON-ZERO origin window at plane (8, 4), size 40×40.
    engine.resize_canvas(CanvasRect::from_xywh(8, 4, 40, 40));

    // Screen center → window-local center (20, 20) → plane (8+20, 4+20).
    let (px, py) = engine.screen_to_plane(sw / 2.0, sh / 2.0);
    assert!(
        (px - 28.0).abs() < 1e-2,
        "screen center must map to plane x = origin.x + window_center (28), got {px}"
    );
    assert!(
        (py - 24.0).abs() < 1e-2,
        "screen center must map to plane y = origin.y + window_center (24), got {py}"
    );
}

/// Bounding box of all marching-ants (`KIND_DASHED_LINE`, `FLAG_CANVAS_SPACE`)
/// vertices — returned as a plane-space `(min_x, min_y, max_x, max_y)`.
fn ants_bbox(prims: &[OverlayPrimitive]) -> (f32, f32, f32, f32) {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut found = false;
    for p in prims {
        if p.kind != KIND_DASHED_LINE || (p.flags & FLAG_CANVAS_SPACE) == 0 {
            continue;
        }
        for &[x, y] in &[p.p0, p.p1] {
            found = true;
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
    }
    assert!(found, "no marching-ants primitives present");
    (minx, miny, maxx, maxy)
}

/// MARCHING-ANTS regression: the selection outline is pushed as plane-space
/// (`FLAG_CANVAS_SPACE`) overlay primitives, but the contours are extracted from
/// the **window-local** selection texture. Without lifting them by `canvas_origin`
/// the ants render short of the selection by the crop offset. After a non-zero
/// crop the ant bbox must coincide with the selection's **plane** rect — not its
/// window-local rect.
#[test]
fn marching_ants_track_selection_plane_bounds_after_crop() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    // Crop to a NON-ZERO origin window at plane (8, 4), size 40×40.
    engine.resize_canvas(CanvasRect::from_xywh(8, 4, 40, 40));

    // Select a plane rect fully inside the window. `select_rect` rasterizes into
    // the window-local mask and `generate_contours_from_mask` emits the ants
    // synchronously (Replace path), so the overlay is populated immediately.
    engine.select_rect(16.0, 12.0, 10.0, 8.0, SelectionMode::Replace, false, 0.0);

    let (minx, miny, maxx, maxy) = ants_bbox(&engine.test_selection_overlay());
    // Plane rect is [16, 12]..[26, 20]; allow a couple px of contour padding.
    // The pre-fix bug placed the bbox at the window-local [8, 8]..[18, 16].
    assert!(
        (minx - 16.0).abs() <= 2.0 && (miny - 12.0).abs() <= 2.0,
        "ants min ({minx}, {miny}) must track the selection's PLANE origin (16, 12), \
         not the window-local (8, 8)"
    );
    assert!(
        (maxx - 26.0).abs() <= 2.0 && (maxy - 20.0).abs() <= 2.0,
        "ants max ({maxx}, {maxy}) must track the selection's PLANE far edge (26, 20)"
    );
}

/// TRANSFORM-SOURCE regression: `begin_transform` from a selection must hand
/// `setup_transform` a **plane-space** source origin (matching the no-selection
/// branch's `layer_to_canvas`). Selection bounds are window-local; feeding them
/// raw offsets the picked-up region by `canvas_origin`.
#[test]
fn transform_from_selection_uses_plane_source_origin() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);

    // Paint so the layer has content under the selection (begin_transform on a
    // selection does not require content, but keep it realistic).
    paint_row(&mut engine, layer_id, 16.0, 8.0, 40.0);

    engine.resize_canvas(CanvasRect::from_xywh(8, 4, 40, 40));
    engine.select_rect(16.0, 12.0, 10.0, 8.0, SelectionMode::Replace, false, 0.0);

    assert!(engine.begin_transform(layer_id), "transform should set up");
    let (sox, soy, sw, sh, _m) = engine.floating_info().expect("floating active");
    assert!(
        (sox - 16.0).abs() <= 1.0 && (soy - 12.0).abs() <= 1.0,
        "source origin ({sox}, {soy}) must be the selection's PLANE origin (16, 12), \
         not the window-local (8, 8)"
    );
    assert!(
        (sw - 10.0).abs() <= 1.0 && (sh - 8.0).abs() <= 1.0,
        "source size ({sw}, {sh}) should match the selection (10, 8)"
    );
}

/// TRANSFORM-PREVIEW regression: entering a transform with the identity matrix
/// is a visual no-op — the live target was only copied out, then re-stamped in
/// the same place. After a non-zero crop the preview is built on a window-sized
/// texture; if its copy/target frames disagree with where the host composites it
/// (`layer_offset = canvas_origin`), the content jumps by the crop offset on
/// enter. The presented viewport during the (identity) transform must match the
/// pre-transform present.
#[test]
fn transform_preview_matches_pretransform_present_after_crop() {
    let (w, h) = (96u32, 96u32);
    let (vw, vh) = (192u32, 192u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, vw as f32, vh as f32);

    // Crop to a non-zero origin, then paint a small mark at the window center.
    engine.resize_canvas(CanvasRect::from_xywh(24, 16, 48, 48));
    paint_cross(&mut engine, layer_id, 48.0, 40.0, 6.0);

    // Baseline present (no floating).
    let baseline = engine.test_readback_viewport(vw, vh);
    let (bx, by) = red_centroid(&baseline, vw, vh);

    // Select the mark and enter an identity transform. The presented preview
    // must keep the mark at the same screen position.
    engine.select_rect(40.0, 32.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    assert!(engine.begin_transform(layer_id), "transform should set up");

    let preview = engine.test_readback_viewport(vw, vh);
    let (px, py) = red_centroid(&preview, vw, vh);

    assert!(
        (px - bx).abs() <= 2.0 && (py - by).abs() <= 2.0,
        "identity-transform preview moved the content: baseline centroid ({bx:.1}, {by:.1}) \
         vs preview ({px:.1}, {py:.1}) — a `canvas_origin` shift in the floating preview frame"
    );
}

/// Centroid (in viewport px) of clearly-red pixels.
fn red_centroid(px: &[u8], w: u32, h: u32) -> (f32, f32) {
    let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0u32);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if px[i] > 128 && px[i + 1] < 80 && px[i + 2] < 80 {
                sx += x as f32;
                sy += y as f32;
                n += 1;
            }
        }
    }
    assert!(n > 0, "no red pixels found");
    (sx / n as f32, sy / n as f32)
}

/// COPY regression: copying a selection in a cropped document must extract the
/// pixels under the selection's **plane** location and stamp the clipboard with
/// a **plane** offset (so paste-in-place lands it back correctly). The copy
/// `region` is window-local; the pre-fix code fed it raw to the layer read (so it
/// grabbed pixels offset by `-canvas_origin`) and to the clipboard offset (so
/// paste landed offset by `-canvas_origin`).
#[test]
fn copy_selection_after_crop_extracts_plane_pixels_and_offset() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);

    // Full-canvas layer with a solid red 8×8 square at PLANE (30, 28).
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 28..36 {
        for x in 30..38 {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = 255; // R
            rgba[i + 3] = 255; // A
        }
    }
    let layer_id = engine.paste_image(w, h, &rgba, 0, 0, None);

    // Crop to a NON-ZERO origin window at plane (16, 12); the square is inside.
    engine.resize_canvas(CanvasRect::from_xywh(16, 12, 40, 40));

    // Select the square in plane coords, then copy.
    engine.select_rect(30.0, 28.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);
    engine.copy(layer_id);

    // Drive the async copy readback to completion.
    let mut export = None;
    for _ in 0..8 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if let Some(e) = engine.poll_copy_result() {
            export = Some(e);
            break;
        }
    }
    let export = export.expect("copy readback should complete");

    // Clipboard offset is the PLANE position of the selection (30, 28) — not the
    // window-local (14, 16) the pre-fix code produced.
    assert_eq!(
        (export.offset_x, export.offset_y),
        (30, 28),
        "clipboard offset must be the selection's plane position"
    );
    // And the extracted pixels are the red square — proving the layer read came
    // from the right plane location, not from the empty (14, 16) region.
    let any_red = export
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[0] > 200 && p[1] < 60 && p[2] < 60 && p[3] > 200);
    assert!(
        any_red,
        "copied pixels must be the red square; an offset layer read would grab empty space"
    );
}

/// TRANSFORM-WITH-SELECTION regression: setting up a selection transform crops
/// the (window-local) selection cpu-cache at the source origin to mask the
/// picked-up pixels. The source origin handed to `setup_transform` is PLANE
/// (for the live-layer pickup + the clear rect), so the selection-crop read must
/// shift it back to window-local. If it doesn't, after a crop the cropped mask
/// is all-zero → the transform picks up nothing and an identity commit ERASES
/// the selected content instead of preserving it.
#[test]
fn identity_transform_with_selection_preserves_content_after_crop() {
    let (w, h) = (96u32, 96u32);
    let mut engine = test_engine(w, h);

    // Full-canvas layer with a solid red 16×16 square at PLANE (40, 32).
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 32..48 {
        for x in 40..56 {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = 255;
            rgba[i + 3] = 255;
        }
    }
    let layer_id = engine.paste_image(w, h, &rgba, 0, 0, None);

    // Crop to a NON-ZERO origin window (24, 16) that contains the square.
    engine.resize_canvas(CanvasRect::from_xywh(24, 16, 48, 48));

    // Select the square (plane coords) and run an IDENTITY transform — a no-op
    // that must leave the content exactly where it was.
    engine.select_rect(40.0, 32.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    assert!(engine.begin_transform(layer_id), "transform should set up");
    engine.commit_floating();
    engine.render(0.0);

    // Layer is plane-anchored at origin 0, so plane == texel. The square must
    // survive the identity round-trip.
    let px = engine.test_readback_layer(layer_id);
    assert!(
        alpha_at(&px, w, 47, 40) > 0 && alpha_at(&px, w, 41, 33) > 0,
        "identity transform-with-selection erased the content after a crop \
         (cropped selection mask read at the wrong frame)"
    );
}

/// COLOR-PICK regression: the color picker receives plane coords (the frontend's
/// `screenToCanvas`). The merged composite is window-local, so a merged pick must
/// sample `plane − canvas_origin`. The pre-fix code sampled the composite at the
/// raw plane texel (and bounds-checked plane against the window size), so after a
/// crop a merged pick read the wrong pixel — or fell outside the window and
/// returned black.
#[test]
fn pick_color_merged_reads_plane_pixel_after_crop() {
    use darkly::engine::PickSource;

    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);

    // Full-canvas layer: red everywhere, with a distinct green 4×4 at plane (40, 40).
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for px in rgba.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[200, 0, 0, 255]);
    }
    for y in 40..44 {
        for x in 40..44 {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[0, 200, 0, 255]);
        }
    }
    let _layer = engine.paste_image(w, h, &rgba, 0, 0, None);

    // Crop to a NON-ZERO origin window (16, 12); plane (40, 40) is inside it.
    engine.resize_canvas(CanvasRect::from_xywh(16, 12, 40, 40));
    // Force an offscreen composite so the merged texture is populated.
    let _ = engine.test_readback_canvas();

    // Merged pick at the green pixel's PLANE coordinate.
    engine.pick_color(41.0, 41.0, PickSource::Merged);
    engine.test_flush_readbacks();
    let c = engine.last_picked_color();
    assert!(
        c[1] > 150 && c[0] < 80,
        "merged pick after crop must read the plane pixel (green); got {c:?} \
         (pre-fix read the wrong composite texel or returned black)"
    );
}

/// SELECTION-CACHE-STALENESS regression: a crop re-realizes the selection mask
/// at the moved window (preserving its plane pixels), but the doc-side
/// window-local caches (`pixel_bounds` / `cpu_cache`) were measured against the
/// OLD window. They must be invalidated and repopulated, or a transform/copy
/// issued after the crop picks up the wrong region (the stale window-local
/// bounds lifted by the NEW `canvas_origin`).
#[test]
fn selection_bounds_repopulate_after_crop() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);

    // Red 8×8 square at PLANE (40, 40), selected while the doc is un-cropped.
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 40..48 {
        for x in 40..48 {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = 255;
            rgba[i + 3] = 255;
        }
    }
    let layer_id = engine.paste_image(w, h, &rgba, 0, 0, None);
    engine.select_rect(40.0, 40.0, 8.0, 8.0, SelectionMode::Replace, false, 0.0);

    // Crop to a non-zero origin window; the selection's PLANE position (40, 40)
    // is preserved, but its window-local position changes to (24, 28).
    engine.resize_canvas(CanvasRect::from_xywh(16, 12, 40, 40));

    // Drive the readback the crop kicked, repopulating the window-local caches.
    for _ in 0..8 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if engine.test_selection_cpu_cache().is_some() {
            break;
        }
    }

    // begin_transform must pick up the square at its PLANE position (40, 40),
    // not the stale (40 + 16, 40 + 12) = (56, 52).
    assert!(engine.begin_transform(layer_id), "transform should set up");
    let (sox, soy, _, _, _) = engine.floating_info().expect("floating active");
    assert!(
        (sox - 40.0).abs() <= 2.0 && (soy - 40.0).abs() <= 2.0,
        "post-crop selection bounds are stale: transform source ({sox}, {soy}) \
         should be the plane position (40, 40)"
    );
}

/// Crop-to-selection sets the canvas window to the selection's plane bounds.
#[test]
fn crop_to_selection_matches_selection_bounds() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let _layer = engine.add_raster_layer(None);

    // Selection rect at plane (16, 12), size 20×24.
    engine.select_rect(16.0, 12.0, 20.0, 24.0, SelectionMode::Replace, false, 0.0);
    // Populate the selection's pixel bounds from the CPU cache so the crop has
    // something to read without waiting on the async readback.
    engine.crop_to_selection();

    assert_eq!(engine.canvas_rect(), CanvasRect::from_xywh(16, 12, 20, 24));
}

/// Paint a red `+` into `layer`: a horizontal and a vertical arm of equal plane
/// length `2*arm`, crossing at plane (cx, cy). The presented width vs height of
/// the red is a brush-size-independent probe of per-axis scale — an isotropic
/// present yields equal arms, an anisotropic (squashed) present unequal arms.
fn paint_cross(engine: &mut DarklyEngine, layer_id: LayerId, cx: f32, cy: f32, arm: f32) {
    for horizontal in [true, false] {
        engine.begin_stroke(layer_id).unwrap();
        let steps = 48;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = if horizontal {
                (cx - arm + 2.0 * arm * t, cy)
            } else {
                (cx, cy - arm + 2.0 * arm * t)
            };
            engine.stroke_to(StrokeOp::BrushStroke {
                x,
                y,
                pressure: 1.0,
                x_tilt: 0.0,
                y_tilt: 0.0,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: i as f64 * 16.0,
                cr: 1.0,
                cg: 0.0,
                cb: 0.0,
                ca: 1.0,
            });
        }
        engine.end_stroke();
    }
}

/// Bounding-box `(width, height)` of clearly-red pixels in an RGBA8 buffer.
fn red_extent(px: &[u8], w: u32, h: u32) -> (u32, u32) {
    let (mut minx, mut miny, mut maxx, mut maxy) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut found = false;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if px[i] > 128 && px[i + 1] < 80 && px[i + 2] < 80 {
                found = true;
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
    }
    assert!(found, "no red cross found");
    (maxx - minx + 1, maxy - miny + 1)
}

/// Presented horizontal:vertical arm ratio of an equal-armed cross painted at
/// the center of the (optionally cropped) canvas window, viewed through the
/// PRODUCTION view transform. For an isotropic screen↔canvas mapping this is
/// ~1.0 regardless of crop; an anisotropic (squashed) mapping skews it. Soft-
/// edge sampling noise is constant across crops, so the invariant is "the ratio
/// does not change when you crop", not "the ratio is exactly 1".
fn presented_cross_ratio(crop: Option<CanvasRect>) -> f32 {
    // Large canvas/window vs a small cross so the dab never clips the window
    // (clipping would itself skew the ratio toward the window aspect and
    // masquerade as squash).
    let (w, h) = (200u32, 200u32);
    let (vw, vh) = (400u32, 400u32);
    let mut engine = test_engine(w, h);
    let layer_id = engine.add_raster_layer(None);
    // Production-like view: viewport 400, zoom 1, no pan/rotation/mirror.
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, vw as f32, vh as f32);
    // resize_canvas rebuilds the view transform internally (production path);
    // the frontend $effect does NOT re-push set_view_transform on crop.
    let center = match crop {
        Some(rect) => {
            engine.resize_canvas(rect);
            // Paint at the window center so the small cross stays well inside.
            (
                rect.origin.x as f32 + rect.width as f32 / 2.0,
                rect.origin.y as f32 + rect.height as f32 / 2.0,
            )
        }
        None => (100.0, 100.0),
    };
    paint_cross(&mut engine, layer_id, center.0, center.1, 8.0);
    let px = engine.test_readback_viewport(vw, vh);
    let (cw, ch) = red_extent(&px, vw, vh);
    cw as f32 / ch as f32
}

/// REGRESSION (Round 1-3 squash, "even the dabs are squashed sidewise"):
/// cropping to a non-zero origin with a changed aspect ratio must NOT
/// anisotropically squash the presented image. Probed via the PRODUCTION view
/// transform (`test_readback_viewport`) — the identity / composite-cache /
/// layer readbacks are all blind to this (they showed false-green for two
/// fix rounds).
#[test]
fn crop_does_not_anisotropically_squash_presented_content() {
    let uncropped = presented_cross_ratio(None);
    // Non-zero origin (40,30) AND a non-1 aspect ratio (160:100 = 1.6).
    let cropped = presented_cross_ratio(Some(CanvasRect::from_xywh(40, 30, 160, 100)));
    let rel = (cropped / uncropped - 1.0).abs();
    assert!(
        rel < 0.08,
        "crop squashed the presented image: arm ratio {cropped:.3} vs uncropped {uncropped:.3} \
         ({:.0}% change)",
        rel * 100.0
    );
}

/// REGRESSION: the squash must not COMPOUND across successive crops (the user
/// reports each resize/crop "progressively fucks up the situation even more").
#[test]
fn successive_crops_do_not_compound_squash() {
    let uncropped = presented_cross_ratio(None);
    let one = presented_cross_ratio(Some(CanvasRect::from_xywh(40, 30, 160, 100)));
    // A second crop nested inside the first, still containing the window-center
    // cross with margin.
    let (vw, vh) = (400u32, 400u32);
    let mut engine = test_engine(200, 200);
    let layer_id = engine.add_raster_layer(None);
    engine.set_view_transform(0.0, 0.0, 1.0, 0.0, false, vw as f32, vh as f32);
    engine.resize_canvas(CanvasRect::from_xywh(40, 30, 160, 100));
    engine.resize_canvas(CanvasRect::from_xywh(60, 50, 110, 70));
    paint_cross(&mut engine, layer_id, 115.0, 85.0, 8.0);
    let px = engine.test_readback_viewport(vw, vh);
    let (cw, ch) = red_extent(&px, vw, vh);
    let two = cw as f32 / ch as f32;
    assert!(
        (one / uncropped - 1.0).abs() < 0.08 && (two / uncropped - 1.0).abs() < 0.08,
        "squash compounds across crops: uncropped {uncropped:.3}, 1 crop {one:.3}, 2 crops {two:.3}"
    );
}

/// REGRESSION: a present dropped by a `Lost`/`Outdated` surface acquire must
/// keep the frame loop alive. On resize, the compositor's acquire can return
/// `Outdated`; that path reconfigures the surface and returns *without*
/// presenting, leaving `needs_present` set. If `render`'s returned `needs_more`
/// ignores that pending present, JS never reschedules and the reconfigured
/// surface never gets a real frame — a stale/frozen canvas. `needs_more` must
/// therefore surface `compositor.needs_present()`.
#[test]
fn dropped_present_keeps_requesting_frames() {
    let mut engine = test_engine(64, 64);

    // Drain startup async work (thumbnail readbacks, etc.), then clear the
    // pending-present flag so the baseline is genuinely quiescent — headless
    // renders never reach `finish_present`, so `needs_present` would otherwise
    // stay stuck set from engine setup.
    for _ in 0..8 {
        engine.render(0.0);
    }
    engine.test_clear_needs_present();
    assert!(
        !engine.test_frame_needs_more(),
        "engine did not settle to quiescence; test baseline is unreliable"
    );

    // Mimic the compositor's `Lost`/`Outdated` early-return: a present is owed
    // but was dropped without `finish_present` clearing the flag.
    engine.test_mark_needs_present();

    assert!(
        engine.test_frame_needs_more(),
        "a pending present must keep the frame loop alive so the dropped frame reschedules"
    );
}

// ---------------------------------------------------------------------------
// Effect-instance invalidation. An instance's bind groups point at the pair it
// was prepared against — a group accumulator in canvas space, the run's
// ping-pong in screen space. Both are replaced under it: the canvas rect
// recreates every accumulator, and a viewport resize recreates the run's
// textures, which `set_canvas_rect` never touches. A missed invalidation leaves
// a bind group pointing at a freed texture.
// ---------------------------------------------------------------------------

/// Add an effect layer at the top of the stack.
fn effect_layer(engine: &mut DarklyEngine, pipeline: &str) -> LayerId {
    let defaults: Vec<_> = engine
        .filter_param_defs(pipeline)
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect();
    engine
        .add_filter_layer(pipeline, defaults, None)
        .expect("effect layer is addable")
}

fn fill(engine: &mut DarklyEngine, layer_id: LayerId, r: u8, g: u8, b: u8) {
    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r,
        g,
        b,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    engine.test_flush_readbacks();
    engine.render(0.0);
}

fn rgba_at(pixels: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// A canvas-space effect layer keeps working after the canvas rect moves.
#[test]
fn effect_layer_survives_canvas_resize() {
    let mut engine = test_engine(16, 16);
    let red = engine.add_raster_layer(None);
    fill(&mut engine, red, 255, 0, 0);
    let _inv = effect_layer(&mut engine, "invert");
    engine.test_flush_readbacks();
    engine.render(0.0);
    assert_eq!(
        rgba_at(&engine.test_readback_canvas(), 16, 8, 8),
        [0, 255, 255, 255],
        "baseline: the effect inverts before the resize"
    );

    engine.resize_canvas(CanvasRect::new(CanvasPoint::new(0, 0), 32, 32));
    engine.test_flush_readbacks();
    engine.render(0.0);

    assert_eq!(
        rgba_at(&engine.test_readback_canvas(), 32, 4, 4),
        [0, 255, 255, 255],
        "the effect must still invert against the recreated accumulators"
    );
}

/// Moving an effect across the divider re-prepares it against the other
/// space's pair — the second invalidation trigger, and the one no canvas-rect
/// change can stand in for.
#[test]
fn effect_layer_survives_crossing_the_divider() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);
    let red = engine.add_raster_layer(None);
    fill(&mut engine, red, 255, 0, 0);
    let _inv = effect_layer(&mut engine, "invert");

    engine.set_screen_space_boundary(1);
    engine.test_flush_readbacks();
    engine.render(0.0);
    let screen = engine.test_readback_screen_run(cw, ch);
    assert_eq!(
        rgba_at(&screen, cw, 8, 8),
        [0, 255, 255, 255],
        "prepared against the run's pair"
    );

    engine.set_screen_space_boundary(0);
    engine.test_flush_readbacks();
    engine.render(0.0);
    assert_eq!(
        rgba_at(&engine.test_readback_canvas(), cw, 8, 8),
        [0, 255, 255, 255],
        "and re-prepared against the accumulator on the way back"
    );
}

/// A viewport resize replaces the run's textures. `set_canvas_rect` does not
/// touch them, so this is the one invalidation trigger the canvas-rect path
/// cannot cover.
#[test]
fn screen_space_effect_survives_viewport_resize() {
    let (cw, ch) = (16u32, 16u32);
    let mut engine = test_engine(cw, ch);
    let red = engine.add_raster_layer(None);
    fill(&mut engine, red, 255, 0, 0);
    let _inv = effect_layer(&mut engine, "invert");
    engine.set_screen_space_boundary(1);

    let first = engine.test_readback_screen_run(cw, ch);
    assert_eq!(rgba_at(&first, cw, 8, 8), [0, 255, 255, 255]);

    // A second read at a different size forces the run's textures to be
    // recreated between the two.
    let grown = engine.test_readback_screen_run(cw * 2, ch * 2);
    assert_eq!(
        rgba_at(&grown, cw * 2, 8, 8),
        [0, 255, 255, 255],
        "the instance must be rebuilt against the resized run textures"
    );
}
