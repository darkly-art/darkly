//! GPU undo region storage: entries hold the tiles an operation changed, and
//! every pending commit keeps the pixels it was saved from.
//!
//! Run with: `cargo test -p darkly --features testing --test undo_tiles -- --test-threads=1`

use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn brush_op(x: f32, y: f32) -> StrokeOp {
    colored_brush_op(x, y, 1.0)
}

/// A brush op whose color is `[red, 0, 0]`; a mask takes the red channel.
fn colored_brush_op(x: f32, y: f32, red: f32) -> StrokeOp {
    StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: red,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    }
}

/// One stroke through `points`, without settling afterwards.
fn stroke(engine: &mut DarklyEngine, layer_id: LayerId, points: &[(f32, f32)]) {
    engine.begin_stroke(layer_id).unwrap();
    for &(x, y) in points {
        engine.stroke_to(brush_op(x, y));
    }
    engine.end_stroke();
}

/// RGBA of `layer_id` at canvas `(x, y)`, wherever the layer's texture sits.
fn pixel_at(engine: &DarklyEngine, layer_id: LayerId, x: i32, y: i32) -> [u8; 4] {
    let extent = engine.test_node_extent(layer_id);
    let pixels = engine.test_readback_layer(layer_id);
    let lx = (x - extent.origin.x) as u32;
    let ly = (y - extent.origin.y) as u32;
    let i = ((ly * extent.width + lx) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

/// Regression: an explicit-rect operation between a stroke's `end_stroke`
/// and its deferred commit overwrote the shared scratch, and the forced
/// flush then committed the stroke from it, so undoing the stroke restored
/// another layer's pixels.
#[test]
fn pending_stroke_undo_survives_an_explicit_save() {
    let mut engine = test_engine(256, 256);
    let a = engine.add_raster_layer(None);
    let b = engine.add_raster_layer(None);
    engine.fill_background_color(b, [0, 255, 0, 255]);
    engine.test_settle_pending();

    stroke(&mut engine, a, &[(100.0, 100.0), (140.0, 140.0)]);
    // No frame in between: the stroke's commit is still pending.
    engine.fill_background_color(b, [0, 0, 255, 255]);
    engine.test_settle_pending();

    engine.undo();
    engine.test_settle_pending();
    engine.undo();
    engine.test_settle_pending();
    assert_eq!(
        pixel_at(&engine, a, 120, 120),
        [0, 0, 0, 0],
        "undoing the stroke restores layer A's own pre-stroke pixels"
    );
}

/// Regression: a commit translated canvas coords through the layer's live
/// frame, so a second stroke that grew the layer up and left before the
/// first stroke's commit made that commit read the wrong texels.
#[test]
fn pending_stroke_undo_survives_layer_growth() {
    let mut engine = test_engine(256, 256);
    let layer_id = engine.add_raster_layer(None);
    engine.fill_background_color(layer_id, [0, 255, 0, 255]);
    engine.test_settle_pending();

    stroke(&mut engine, layer_id, &[(100.0, 100.0), (120.0, 120.0)]);
    // No frame in between: this stroke grows the layer while the first
    // stroke's commit is still pending.
    stroke(&mut engine, layer_id, &[(-60.0, 200.0), (-40.0, 200.0)]);
    engine.test_settle_pending();
    assert!(
        engine.test_node_extent(layer_id).origin.x < 0,
        "the layer grew left"
    );

    engine.undo();
    engine.test_settle_pending();
    engine.undo();
    engine.test_settle_pending();
    assert_eq!(
        pixel_at(&engine, layer_id, 110, 110),
        [0, 255, 0, 255],
        "undoing the first stroke restores the green it painted over"
    );
}

/// Bytes of a single-rect entry covering `w × h` RGBA8 pixels.
fn rgba_rect_bytes(w: u32, h: u32) -> u64 {
    (w * 4).div_ceil(256) as u64 * 256 * h as u64
}

/// Undo and redo of a multi-run entry put back exactly the pixels each side
/// held, over painted content and with untouched pixels between the runs.
#[test]
fn multi_run_entry_undoes_and_redoes_exactly() {
    let mut engine = test_engine(1024, 1024);
    let layer_id = engine.add_raster_layer(None);
    engine.fill_background_color(layer_id, [0, 255, 0, 255]);
    engine.test_settle_pending();
    let before = engine.test_readback_layer(layer_id);
    let bytes0 = engine.test_undo_total_bytes();

    let diagonal: Vec<(f32, f32)> = (0..=20)
        .map(|i| (60.0 + i as f32 * 45.0, 60.0 + i as f32 * 45.0))
        .collect();
    stroke(&mut engine, layer_id, &diagonal);
    engine.test_settle_pending();
    let after = engine.test_readback_layer(layer_id);
    assert!(
        engine.test_undo_total_bytes() - bytes0 < rgba_rect_bytes(900, 900) / 2,
        "the entry is stored as runs, not its bounding rect"
    );

    engine.undo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_layer(layer_id) == before,
        "undo is exact"
    );
    engine.redo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_layer(layer_id) == after,
        "redo is exact"
    );
}

/// Runs through the partial tiles at the right and bottom edges of a canvas
/// that is not a multiple of the tile size restore exactly.
#[test]
fn partial_edge_tiles_restore_exactly() {
    let mut engine = test_engine(1000, 1000);
    let layer_id = engine.add_raster_layer(None);
    engine.fill_background_color(layer_id, [0, 255, 0, 255]);
    engine.test_settle_pending();
    let before = engine.test_readback_layer(layer_id);
    let bytes0 = engine.test_undo_total_bytes();

    stroke(
        &mut engine,
        layer_id,
        &[(995.0, 10.0), (995.0, 995.0), (10.0, 995.0)],
    );
    engine.test_settle_pending();
    let after = engine.test_readback_layer(layer_id);
    assert!(
        engine.test_undo_total_bytes() - bytes0 < rgba_rect_bytes(1000, 1000) / 2,
        "an L along two edges is stored as runs"
    );

    engine.undo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_layer(layer_id) == before,
        "undo is exact"
    );
    engine.redo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_layer(layer_id) == after,
        "redo is exact"
    );
}

/// A stroke that grows the layer mid-stroke still stores only what it
/// changed, and undo restores the pre-stroke pixels at their canvas position.
#[test]
fn stroke_that_grows_the_layer_stores_its_tiles() {
    let mut engine = test_engine(256, 256);
    let layer_id = engine.add_raster_layer(None);
    engine.fill_background_color(layer_id, [0, 255, 0, 255]);
    engine.test_settle_pending();
    let bytes0 = engine.test_undo_total_bytes();

    stroke(&mut engine, layer_id, &[(100.0, 100.0), (-60.0, 100.0)]);
    engine.test_settle_pending();
    let grown = engine.test_node_extent(layer_id);
    assert!(grown.origin.x < 0, "the layer grew left");
    assert!(
        engine.test_undo_total_bytes() - bytes0 < rgba_rect_bytes(grown.width, grown.height) / 2,
        "the entry costs the stroke's tiles, not the grown extent"
    );

    engine.undo();
    engine.test_settle_pending();
    assert_eq!(pixel_at(&engine, layer_id, 50, 100), [0, 255, 0, 255]);
    assert_eq!(pixel_at(&engine, layer_id, -40, 100), [0, 0, 0, 0]);
}

/// R8 masks diff and store on their own 256x64 grid.
#[test]
fn mask_stroke_restores_exactly() {
    let mut engine = test_engine(1024, 1024);
    let layer_id = engine.add_raster_layer(None);
    engine.add_mask(layer_id).expect("add mask");
    let mask_id = engine.host_mask_id(layer_id).unwrap();
    engine.test_settle_pending();
    let before = engine.test_readback_mask(layer_id);
    let len0 = engine.test_undo_len();

    // Black: a fresh mask is white, so a white stroke would change nothing.
    engine.begin_stroke(mask_id).unwrap();
    engine.stroke_to(colored_brush_op(100.0, 100.0, 0.0));
    engine.stroke_to(colored_brush_op(900.0, 900.0, 0.0));
    engine.end_stroke();
    engine.test_settle_pending();
    assert_eq!(engine.test_undo_len(), len0 + 1);
    assert!(
        engine.test_readback_mask(layer_id) != before,
        "the stroke changed the mask"
    );

    engine.undo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_mask(layer_id) == before,
        "undo is exact"
    );
}

/// Two strokes with no frame between them each get their own tile-sized
/// entry: the second stroke's save no longer forces the first to commit its
/// whole saved extent.
#[test]
fn consecutive_strokes_each_get_a_tile_sized_entry() {
    let mut engine = test_engine(1024, 1024);
    let layer_id = engine.add_raster_layer(None);
    engine.fill_background_color(layer_id, [0, 255, 0, 255]);
    engine.test_settle_pending();
    let before = engine.test_readback_layer(layer_id);
    let (len0, bytes0) = (engine.test_undo_len(), engine.test_undo_total_bytes());

    stroke(&mut engine, layer_id, &[(60.0, 60.0), (960.0, 960.0)]);
    stroke(&mut engine, layer_id, &[(960.0, 60.0), (60.0, 960.0)]);
    engine.test_settle_pending();
    assert_eq!(engine.test_undo_len(), len0 + 2);
    assert!(
        engine.test_undo_total_bytes() - bytes0 < rgba_rect_bytes(1024, 1024),
        "neither entry is the whole saved extent"
    );

    engine.undo();
    engine.test_settle_pending();
    engine.undo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_layer(layer_id) == before,
        "both undos are exact"
    );
}

/// An undoable action inside a stroke's diff window lands after the stroke
/// on the stack, matching the order the artist did them.
#[test]
fn action_inside_the_diff_window_stacks_after_the_stroke() {
    let mut engine = test_engine(256, 256);
    let layer_id = engine.add_raster_layer(None);
    engine.test_settle_pending();
    let before = engine.test_readback_layer(layer_id);
    let len0 = engine.test_undo_len();

    stroke(&mut engine, layer_id, &[(50.0, 50.0), (200.0, 200.0)]);
    // No frame in between: the stroke's diff is still in flight.
    engine.set_opacity(layer_id, 0.5);
    engine.test_settle_pending();
    assert_eq!(engine.test_undo_len(), len0 + 2);

    engine.undo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_layer(layer_id) != before,
        "the first undo reverts the opacity change, not the stroke"
    );
    engine.undo();
    engine.test_settle_pending();
    assert!(
        engine.test_readback_layer(layer_id) == before,
        "the second undo reverts the stroke"
    );
}

/// A flood fill that lands after a later stroke grew the same layer diffs
/// against its snapshot re-anchored to the grown layer, and undoing both
/// returns the layer to its original pixels.
#[test]
fn fill_landing_after_its_layer_grew_commits_cleanly() {
    let mut engine = test_engine(256, 256);
    let layer_id = engine.add_raster_layer(None);
    engine.fill_background_color(layer_id, [0, 255, 0, 255]);
    engine.test_settle_pending();
    let len0 = engine.test_undo_len();

    engine.begin_stroke(layer_id).unwrap();
    engine.stroke_to(StrokeOp::FloodFill {
        x: 100.0,
        y: 100.0,
        r: 0,
        g: 0,
        b: 255,
        a: 255,
        tolerance: 0,
    });
    engine.end_stroke();
    // Before the fill's readback lands, a stroke grows the layer left.
    stroke(&mut engine, layer_id, &[(50.0, 200.0), (-60.0, 200.0)]);
    engine.test_settle_pending();
    assert!(
        engine.test_node_extent(layer_id).origin.x < 0,
        "the layer grew left"
    );
    assert_eq!(
        engine.test_undo_len(),
        len0 + 2,
        "the stroke and the fill each push an entry"
    );

    engine.undo();
    engine.test_settle_pending();
    engine.undo();
    engine.test_settle_pending();
    assert_eq!(pixel_at(&engine, layer_id, 100, 100), [0, 255, 0, 255]);
    assert_eq!(pixel_at(&engine, layer_id, -40, 200), [0, 0, 0, 0]);
}
