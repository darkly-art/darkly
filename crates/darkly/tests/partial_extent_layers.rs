//! Regression tests for nodes whose texture extent does not cover the canvas
//! window: a paste-extent layer, a layer the window was moved off of by a crop,
//! a mask on such a host, and a layer that grew between an edit and its undo.
//!
//! The model these defend: a node's content is defined everywhere. Inside its
//! texture it is the texels; outside it is the node's uncovered value
//! (transparent for a layer, white for a mask, unselected for the selection).
//! The extent is storage, never a boundary an edit can see.
//!
//! Run with: `cargo test -p darkly --features testing --test partial_extent_layers -- --test-threads=1`

mod common;

use darkly::coord::{CanvasPoint, CanvasRect};
use darkly::document::SelectionMode;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

const W: u32 = 64;
const H: u32 = 64;
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// The default window and a cropped one with a non-zero `canvas_origin`
/// (`docs/coordinate-systems.md`: test with a moved window).
const WINDOWS: [CanvasRect; 2] = [
    CanvasRect::from_xywh(0, 0, W, H),
    CanvasRect::from_xywh(-16, 8, W, H),
];

fn engine() -> DarklyEngine {
    let (device, queue) = test_device();
    DarklyEngine::new(GpuContext::new_headless(device, queue), W, H)
}

fn engine_at(window: CanvasRect) -> DarklyEngine {
    let mut e = engine();
    e.resize_canvas(window);
    e
}

fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    rgba.repeat((w * h) as usize)
}

fn settle(e: &mut DarklyEngine) {
    for _ in 0..8 {
        e.test_flush_readbacks();
        e.render(0.0);
    }
    e.test_settle_pending();
}

/// A 64x96 opaque red layer at plane (24, 0): inside the default window only
/// for x >= 24, spilling past its right and bottom edges (and, for the cropped
/// window, its top edge). The 1920x1536 paste at (652, 0) from the bug report,
/// scaled down.
fn paste_partial(e: &mut DarklyEngine) -> LayerId {
    e.paste_image(64, 96, &solid(64, 96, RED), 24, 0, None)
}

/// The node's content at plane `(x, y)`: its texel inside the extent, its
/// uncovered value (`outside`) beyond it. One byte per pixel for an R8 node.
fn content_at(e: &DarklyEngine, id: LayerId, x: i32, y: i32, outside: &[u8]) -> Vec<u8> {
    let ext = e.test_node_extent(id);
    if !ext.contains(CanvasRect::from_xywh(x, y, 1, 1)) {
        return outside.to_vec();
    }
    let px = e.test_readback_layer(id);
    let bpp = px.len() / (ext.width * ext.height) as usize;
    let i = ((y - ext.y0()) as usize * ext.width as usize + (x - ext.x0()) as usize) * bpp;
    px[i..i + bpp].to_vec()
}

fn rgba_at(e: &DarklyEngine, id: LayerId, x: i32, y: i32) -> [u8; 4] {
    content_at(e, id, x, y, &CLEAR).try_into().unwrap()
}

/// The layer's content over `rect`, row-major RGBA, extent-independent: two
/// layers with the same content compare equal whatever their allocations.
fn content_over(e: &DarklyEngine, id: LayerId, rect: CanvasRect) -> Vec<u8> {
    let mut out = Vec::with_capacity((rect.width * rect.height * 4) as usize);
    for y in rect.y0()..rect.y1() {
        for x in rect.x0()..rect.x1() {
            out.extend_from_slice(&rgba_at(e, id, x, y));
        }
    }
    out
}

/// Plane rect covering the partial layer and the window, for whole-content
/// comparisons across undo.
fn neighbourhood() -> CanvasRect {
    CanvasRect::from_xywh(-32, -8, 128, 112)
}

/// The selection's value at plane `(x, y)`: 0 outside the window.
fn selected_at(e: &DarklyEngine, window: CanvasRect, x: i32, y: i32) -> u8 {
    if !window.contains(CanvasRect::from_xywh(x, y, 1, 1)) {
        return 0;
    }
    let sel = e.test_readback_selection().expect("selection state");
    sel[((y - window.y0()) as u32 * window.width + (x - window.x0()) as u32) as usize]
}

/// REGRESSION: Delete with a rect selection on a layer that only partly covers
/// the canvas committed the whole window as an undo run, which lies outside
/// the layer, and the commit asserted ("snapshot run ... must lie inside the
/// source").
#[test]
fn delete_rect_selection_on_layer_partly_covering_canvas() {
    for window in WINDOWS {
        let mut e = engine_at(window);
        let id = paste_partial(&mut e);
        let before = content_over(&e, id, neighbourhood());
        e.select_rect(30.0, 10.0, 10.0, 10.0, SelectionMode::Replace, false, 0.0);
        settle(&mut e);
        e.clear_selection_contents(id);
        settle(&mut e);

        assert_eq!(rgba_at(&e, id, 35, 15)[3], 0, "selected pixel erased");
        assert_eq!(rgba_at(&e, id, 45, 15), RED, "unselected pixel kept");
        let cleared = content_over(&e, id, neighbourhood());

        e.undo();
        settle(&mut e);
        assert_eq!(content_over(&e, id, neighbourhood()), before, "undo");
        e.redo();
        settle(&mut e);
        assert_eq!(content_over(&e, id, neighbourhood()), cleared, "redo");
    }
}

/// REGRESSION: Select All + Delete erased layer pixels outside the canvas
/// window too: the selection was sampled clamp-to-edge, so every texel past
/// the window read the selection's edge value. The undo entry only covers the
/// window, so those pixels were lost for good.
#[test]
fn delete_select_all_keeps_pixels_outside_the_window() {
    for window in WINDOWS {
        let mut e = engine_at(window);
        let id = paste_partial(&mut e);
        let before = content_over(&e, id, neighbourhood());
        e.select_all();
        settle(&mut e);
        e.clear_selection_contents(id);
        settle(&mut e);

        assert_eq!(rgba_at(&e, id, 30, 20)[3], 0, "inside the window: erased");
        assert_eq!(rgba_at(&e, id, 80, 20), RED, "right of the window: kept");
        assert_eq!(rgba_at(&e, id, 30, 90), RED, "below the window: kept");
        if window.y0() > 0 {
            assert_eq!(rgba_at(&e, id, 30, 4), RED, "above the window: kept");
        }

        e.undo();
        settle(&mut e);
        assert_eq!(content_over(&e, id, neighbourhood()), before, "undo");
    }
}

/// REGRESSION: Delete on a layer wholly outside the window panicked in
/// `save_region` ("rect must overlap the source's canvas extent"). Nothing on
/// the layer is in the window, so it is a no-op with no undo entry.
#[test]
fn delete_on_layer_outside_the_window_is_a_no_op() {
    let mut e = engine();
    let id = e.paste_image(16, 16, &solid(16, 16, RED), 100, 100, None);
    let before = e.test_readback_layer(id);
    e.select_all();
    settle(&mut e);
    let depth = e.test_undo_len();
    e.clear_selection_contents(id);
    settle(&mut e);
    assert_eq!(e.test_undo_len(), depth);
    assert_eq!(e.test_readback_layer(id), before);
}

/// REGRESSION: filling a layer that only partly covers the canvas committed
/// the whole window as an undo run outside the layer (the same assert as
/// Delete), and the fill itself stopped at the layer's allocation instead of
/// covering the canvas.
#[test]
fn fill_background_color_covers_the_whole_window() {
    for window in WINDOWS {
        let mut e = engine_at(window);
        let id = paste_partial(&mut e);
        let before = content_over(&e, id, neighbourhood());
        e.fill_background_color(id, BLUE);
        settle(&mut e);

        assert!(
            content_over(&e, id, window).chunks(4).all(|p| p == BLUE),
            "every window pixel is filled, including those left of the paste"
        );
        assert_eq!(rgba_at(&e, id, 80, 20), RED, "outside the window: kept");

        e.undo();
        settle(&mut e);
        assert_eq!(content_over(&e, id, neighbourhood()), before, "undo");
    }
}

/// REGRESSION: `fill_background` wrote the image at window-local offsets into
/// a texture laid out in layer-local ones, so on a layer not anchored at the
/// window origin the image landed shifted (or out of bounds).
#[test]
fn fill_background_lands_in_the_window() {
    let mut reference = engine();
    let ref_id = reference.add_raster_layer(None);
    reference.fill_background(ref_id);
    settle(&mut reference);
    let expected = content_over(&reference, ref_id, WINDOWS[0]);

    let mut e = engine_at(WINDOWS[1]);
    let id = paste_partial(&mut e);
    e.fill_background(id);
    settle(&mut e);
    assert_eq!(content_over(&e, id, WINDOWS[1]), expected);
}

/// REGRESSION: Cut with no selection on a layer that only partly covers the
/// canvas read back only the overlap but labelled it with the whole window, so
/// the clip claimed more pixels than it carried; the clear then hit the same
/// undo assert as Delete.
#[test]
fn cut_without_selection_on_layer_partly_covering_canvas() {
    for window in WINDOWS {
        let mut e = engine_at(window);
        let id = paste_partial(&mut e);
        let before = content_over(&e, id, neighbourhood());
        e.cut(id);
        settle(&mut e);

        let overlap = window
            .intersect(e.test_node_extent(id))
            .expect("layer overlaps window");
        let clip = e.poll_copy_result().expect("cut produced a clip");
        assert_eq!((clip.width, clip.height), (overlap.width, overlap.height));
        assert_eq!((clip.offset_x, clip.offset_y), (overlap.x0(), overlap.y0()));
        assert!(clip.rgba.chunks(4).all(|p| p == RED));

        assert_eq!(rgba_at(&e, id, 30, 20)[3], 0, "inside the window: cut");
        assert_eq!(rgba_at(&e, id, 80, 20), RED, "outside the window: kept");
        e.undo();
        settle(&mut e);
        assert_eq!(content_over(&e, id, neighbourhood()), before, "undo");
    }
}

/// REGRESSION: the same mislabelled readback on a mask indexed past the end of
/// the R8 buffer while widening it to RGBA (a panic in release builds too).
#[test]
fn copy_mask_without_selection_on_host_partly_covering_canvas() {
    // A mask takes the window it was added in; moving the window afterwards
    // leaves it covering only part of the new one.
    let mut e = engine();
    let host = paste_partial(&mut e);
    e.add_mask(host).expect("add mask");
    let mask = e.test_mask_id(host).expect("mask id");
    e.resize_canvas(WINDOWS[1]);
    settle(&mut e);
    let overlap = WINDOWS[1]
        .intersect(e.test_node_extent(mask))
        .expect("mask overlaps window");
    assert_ne!(overlap, WINDOWS[1], "the mask must not cover the window");
    e.copy(mask);
    settle(&mut e);
    let clip = e.poll_copy_result().expect("copy produced a clip");
    assert_eq!((clip.width, clip.height), (overlap.width, overlap.height));
}

/// REGRESSION: paste-in-place that grew its target (here left and up, from a
/// paste at (40, 40) to the canvas origin) recorded the grown extent as the
/// undo region but also an undo of the growth, so undo shrank the texture and
/// then uploaded the larger region into it.
#[test]
fn paste_in_place_that_grows_the_target_undoes_cleanly() {
    let mut e = engine();
    let src = e.add_raster_layer(None);
    e.fill_background_color(src, BLUE);
    settle(&mut e);
    e.copy(src);
    settle(&mut e);

    let target = e.paste_image(16, 16, &solid(16, 16, RED), 40, 40, None);
    let before = content_over(&e, target, neighbourhood());
    e.paste_in_place(Some(target)).expect("pasted");
    settle(&mut e);
    assert_eq!(rgba_at(&e, target, 4, 4), BLUE, "the paste landed");

    e.undo();
    settle(&mut e);
    assert_eq!(content_over(&e, target, neighbourhood()), before, "undo");
    e.redo();
    settle(&mut e);
    assert_eq!(rgba_at(&e, target, 4, 4), BLUE, "redo");
}

/// REGRESSION: an image rescale redone after the layer grew put the layer back
/// at the extent recorded when the rescale ran, then uploaded the larger
/// region captured at undo time into it.
#[test]
fn rescale_redo_after_the_layer_grew() {
    let mut e = engine();
    let id = e.paste_image(32, 32, &solid(32, 32, RED), 0, 0, None);
    e.rescale_image(32, 32, None);
    settle(&mut e);
    let rescaled = content_over(&e, id, CanvasRect::from_xywh(0, 0, 32, 32));

    // A gradient claims the whole window, growing the 16x16 rescaled layer.
    let grown_from = e.test_node_extent(id);
    e.begin_stroke(id).unwrap();
    e.stroke_to(StrokeOp::LinearGradient {
        x0: 0.0,
        y0: 0.0,
        x1: 32.0,
        y1: 0.0,
        r0: 0,
        g0: 255,
        b0: 0,
        a0: 255,
        r1: 0,
        g1: 255,
        b1: 0,
        a1: 255,
    });
    e.end_stroke();
    settle(&mut e);
    assert_ne!(e.test_node_extent(id), grown_from, "the gradient grew it");

    e.undo(); // the gradient (the growth stays)
    settle(&mut e);
    e.undo(); // the rescale
    settle(&mut e);
    e.redo(); // the rescale
    settle(&mut e);
    assert_eq!(
        content_over(&e, id, CanvasRect::from_xywh(0, 0, 32, 32)),
        rescaled
    );
}

/// REGRESSION: a flood fill clicked on canvas area the layer's texture does
/// not cover did nothing (the fill ran over the texture only, and the seed was
/// outside it), though that area is the layer's transparent content.
#[test]
fn flood_fill_outside_the_layer_texture_fills_the_canvas() {
    for window in WINDOWS {
        let mut e = engine_at(window);
        let id = e.paste_image(16, 16, &solid(16, 16, RED), 0, 16, None);
        e.begin_stroke(id).unwrap();
        e.stroke_to(StrokeOp::FloodFill {
            x: 40.0,
            y: 40.0,
            r: 0,
            g: 0,
            b: 255,
            a: 255,
            tolerance: 0,
        });
        e.end_stroke();
        settle(&mut e);

        assert_eq!(rgba_at(&e, id, 40, 40), BLUE, "clicked area filled");
        assert_eq!(rgba_at(&e, id, window.x0(), window.y1() - 1), BLUE);
        assert_eq!(rgba_at(&e, id, 8, 24), RED, "the paste is a fill border");
    }
}

/// REGRESSION: the magic wand clicked outside the layer's texture selected
/// nothing, though that area is the layer's transparent content.
#[test]
fn magic_wand_outside_the_layer_texture_selects_transparent_area() {
    for window in WINDOWS {
        let mut e = engine_at(window);
        let id = e.paste_image(16, 16, &solid(16, 16, RED), 0, 16, None);
        e.select_magic_wand(id, CanvasPoint::new(40, 40), 0, SelectionMode::Replace);
        settle(&mut e);

        assert_eq!(selected_at(&e, window, 40, 40), 255);
        assert_eq!(selected_at(&e, window, window.x0(), window.y1() - 1), 255);
        assert_eq!(selected_at(&e, window, 8, 24), 0, "the paste is not");
    }
}

/// REGRESSION: loading a mask that only covers part of the canvas as the
/// selection left the rest of the canvas unselected, though a mask reveals
/// (is white) outside its extent. Alpha to selection on the mask had the same
/// gap.
#[test]
fn mask_to_selection_selects_outside_the_mask_extent() {
    let window = WINDOWS[1];
    let mut e = engine();
    let host = e.paste_image(16, 16, &solid(16, 16, RED), 0, 16, None);
    e.add_mask(host).expect("add mask");
    let mask = e.test_mask_id(host).expect("mask id");
    e.resize_canvas(window);
    settle(&mut e);
    // Left of the plane origin: in the moved window, outside the mask, which
    // took the window it was added in.
    let (ox, oy) = (window.x0() + 4, 40);
    assert!(!e
        .test_node_extent(mask)
        .contains(CanvasRect::from_xywh(ox, oy, 1, 1)));

    e.mask_to_selection(mask);
    settle(&mut e);
    assert_eq!(selected_at(&e, window, ox, oy), 255, "mask to selection");

    e.clear_selection();
    e.alpha_to_selection(mask);
    settle(&mut e);
    assert_eq!(selected_at(&e, window, ox, oy), 255, "alpha to selection");
}

/// REGRESSION: a brush stroke under a selection that reaches the window edge
/// painted past the window on a layer that extends there, in the band the
/// clamp-to-edge selection sample leaked into.
#[test]
fn brush_under_a_selection_stops_at_the_window() {
    let mut e = common::new_engine((W, H), "Ink Pen", 0.0);
    let id = e.paste_image(128, 128, &solid(128, 128, CLEAR), -32, -32, None);
    e.select_all();
    settle(&mut e);
    e.set_foreground_color([0.0, 0.0, 1.0, 1.0]);
    common::headless(
        &mut e,
        id,
        &[
            common::event(62.0, 20.0, 0.0),
            common::event(62.0, 44.0, 16.0),
        ],
    );
    settle(&mut e);

    assert!(rgba_at(&e, id, 62, 32)[3] > 0, "painted inside the window");
    for x in W as i32..W as i32 + 8 {
        assert_eq!(
            rgba_at(&e, id, x, 32),
            CLEAR,
            "painted past the window at x={x}"
        );
    }
}

/// REGRESSION: a paint pass bounded by a rect drew one triangle covering up
/// to twice the rect past its far edges, bounded only by the target texture,
/// so filling the window on a layer larger than it also filled a band of the
/// layer beyond the window (outside the undo entry, so for good).
#[test]
fn fill_of_the_window_stops_at_the_window_on_a_larger_layer() {
    let mut e = engine();
    let id = e.paste_image(128, 128, &solid(128, 128, RED), -32, -32, None);
    let before = content_over(&e, id, neighbourhood());
    e.fill_background_color(id, BLUE);
    settle(&mut e);

    assert!(content_over(&e, id, WINDOWS[0])
        .chunks(4)
        .all(|p| p == BLUE));
    assert_eq!(rgba_at(&e, id, 80, 20), RED, "right of the window: kept");
    assert_eq!(rgba_at(&e, id, 20, 80), RED, "below the window: kept");

    e.undo();
    settle(&mut e);
    assert_eq!(content_over(&e, id, neighbourhood()), before, "undo");
}
