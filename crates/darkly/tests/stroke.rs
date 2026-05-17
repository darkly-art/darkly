//! Brush stroke GPU integration tests: stroke workflows, undo/redo, erase.
//!
//! Tests the end-to-end GPU brush flow using raw components (RegionStore,
//! GpuPaintTarget, GpuRegionAction) without a full DarklyEngine.
//! Run with: `cargo test -p darkly --test stroke`

use darkly::coord::CanvasRect;
use darkly::gpu::atlas::CanvasFrame;
use darkly::gpu::diff_rect::DiffRectPass;
use darkly::gpu::paint_target::{GpuPaintTarget, PaintPipelines};
use darkly::gpu::region_store::RegionStore;
use darkly::gpu::test_utils::*;

fn cr(x: i32, y: i32, w: u32, h: u32) -> CanvasRect {
    CanvasRect::from_xywh(x, y, w, h)
}

/// Build a CanvasFrame for a test texture sized `(w, h)` at canvas origin (0, 0).
fn frame<'a>(tex: &'a wgpu::Texture, w: u32, h: u32) -> CanvasFrame<'a> {
    CanvasFrame {
        texture: tex,
        canvas_extent: cr(0, 0, w, h),
    }
}

fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test"),
    })
}

fn submit(queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) {
    queue.submit([encoder.finish()]);
}

fn pixel_at(pixels: &[u8], w: u32, x: u32, y: u32, bpp: u32) -> &[u8] {
    let offset = ((y * w + x) * bpp) as usize;
    &pixels[offset..offset + bpp as usize]
}

// ============================================================================
// End-to-end GPU brush stroke
// ============================================================================

/// Simulate: begin_stroke → paint two circles → end_stroke → undo → redo.
#[test]
fn gpu_stroke_paint_undo_redo() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Create a transparent layer texture.
    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // --- begin_stroke: save full canvas ---
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    // --- stroke_to: two circles ---
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        50.0,
        50.0,
        5.0,
        [255, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        60.0,
        50.0,
        5.0,
        [255, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    // --- end_stroke: commit the stroke rect ---
    // Bounding rect: x=45..65, y=45..55 (approx) — use conservative rect.
    let stroke_rect = cr(43, 43, 24, 14);
    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &frame(&tex, w, h),
        &snap,
        stroke_rect,
    );
    submit(&queue, enc);

    // Verify paint landed.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center1 = pixel_at(&pixels, w, 50, 50, 4);
    assert_eq!(
        center1[0], 255,
        "circle 1 center should be red, R={}",
        center1[0]
    );
    assert_eq!(
        center1[3], 255,
        "circle 1 center should be opaque, A={}",
        center1[3]
    );

    let center2 = pixel_at(&pixels, w, 60, 50, 4);
    assert!(
        center2[3] > 0,
        "circle 2 center should be visible, A={}",
        center2[3]
    );

    let painted_snapshot = pixels.clone();

    // --- undo ---
    let mut enc = encoder(&device);
    let forward_entry = store.restore_region(&mut enc, &entry, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center1 = pixel_at(&pixels, w, 50, 50, 4);
    assert_eq!(
        center1[3], 0,
        "after undo, circle 1 should be gone, A={}",
        center1[3]
    );

    let corner = pixel_at(&pixels, w, 0, 0, 4);
    assert_eq!(corner[3], 0, "corner should still be transparent");

    // --- redo ---
    let mut enc = encoder(&device);
    let _backward_entry = store.restore_region(&mut enc, &forward_entry, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    // The stroke rect area should be pixel-identical to the painted snapshot.
    let (sx, sy, sw, sh) = (
        stroke_rect.x0() as u32,
        stroke_rect.y0() as u32,
        stroke_rect.width,
        stroke_rect.height,
    );
    for y in sy..sy + sh {
        for x in sx..sx + sw {
            let p = pixel_at(&pixels, w, x, y, 4);
            let expected = pixel_at(&painted_snapshot, w, x, y, 4);
            assert_eq!(
                p, expected,
                "redo mismatch at ({x},{y}): got {:?}, expected {:?}",
                p, expected
            );
        }
    }
}

// ============================================================================
// Stroke rect tracking
// ============================================================================

#[test]
fn stroke_rect_tracking() {
    // Test the bounding rect expansion logic (no GPU needed).
    // Simulated GpuStrokeState expand logic (mirrors engine.rs).
    fn expand_rect(
        rect: Option<[u32; 4]>,
        cx: f32,
        cy: f32,
        radius: f32,
        canvas_w: u32,
        canvas_h: u32,
    ) -> [u32; 4] {
        let pad = 2.0;
        let x0 = (cx - radius - pad).max(0.0) as u32;
        let y0 = (cy - radius - pad).max(0.0) as u32;
        let x1 = ((cx + radius + pad).ceil() as u32).min(canvas_w);
        let y1 = ((cy + radius + pad).ceil() as u32).min(canvas_h);

        match rect {
            None => [x0, y0, x1 - x0, y1 - y0],
            Some([sx, sy, sw, sh]) => {
                let nx = sx.min(x0);
                let ny = sy.min(y0);
                let nx1 = (sx + sw).max(x1);
                let ny1 = (sy + sh).max(y1);
                [nx, ny, nx1 - nx, ny1 - ny]
            }
        }
    }

    let canvas = (256, 256);

    // Single circle at (100, 100, r=10).
    let rect = expand_rect(None, 100.0, 100.0, 10.0, canvas.0, canvas.1);
    assert!(rect[0] <= 88, "x0 should be ≤ 88, got {}", rect[0]);
    assert!(rect[1] <= 88, "y0 should be ≤ 88, got {}", rect[1]);
    assert!(
        rect[0] + rect[2] >= 112,
        "x1 should be ≥ 112, got {}",
        rect[0] + rect[2]
    );
    assert!(
        rect[1] + rect[3] >= 112,
        "y1 should be ≥ 112, got {}",
        rect[1] + rect[3]
    );

    // Second circle at (200, 200, r=10) — rect should expand.
    let rect = expand_rect(Some(rect), 200.0, 200.0, 10.0, canvas.0, canvas.1);
    assert!(rect[0] <= 88, "expanded x0 should still cover first circle");
    assert!(
        rect[0] + rect[2] >= 212,
        "expanded x1 should cover second circle"
    );
    assert!(
        rect[1] + rect[3] >= 212,
        "expanded y1 should cover second circle"
    );

    // Circle near edge (0, 0, r=5) — clamped to canvas bounds.
    let rect = expand_rect(None, 0.0, 0.0, 5.0, canvas.0, canvas.1);
    assert_eq!(rect[0], 0, "clamped x0 should be 0");
    assert_eq!(rect[1], 0, "clamped y0 should be 0");

    // Small stroke rect should be << canvas size.
    let small = expand_rect(None, 128.0, 128.0, 3.0, canvas.0, canvas.1);
    assert!(
        small[2] < 20,
        "small stroke width should be < 20, got {}",
        small[2]
    );
    assert!(
        small[3] < 20,
        "small stroke height should be < 20, got {}",
        small[3]
    );
}

// ============================================================================
// GPU stroke on mask (R8)
// ============================================================================

/// Paint black on a fully-revealed mask → undo → verify mask is restored.
#[test]
fn gpu_stroke_on_mask_undo() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::R8Unorm;

    // Fully-revealed mask (255).
    let white = vec![255u8; (w * h) as usize];
    let (tex, view) = create_test_texture_with_format(&device, &queue, w, h, &white, fmt);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // begin_stroke: save full canvas.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    // stroke_to: paint black circles → mask toward 0.
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };

    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        64.0,
        64.0,
        10.0,
        [0, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    // end_stroke: commit.
    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &frame(&tex, w, h),
        &snap,
        cr(52, 52, 24, 24),
    );
    submit(&queue, enc);

    // Verify mask is painted.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center = (64 * w + 64) as usize;
    assert!(
        pixels[center] < 10,
        "mask center should be near 0, got {}",
        pixels[center]
    );
    assert_eq!(pixels[0], 255, "mask corner should be 255");

    // Undo: restore mask to 255.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixels[center], 255,
        "after undo, mask center should be 255, got {}",
        pixels[center]
    );
}

// ============================================================================
// Multiple strokes with undo
// ============================================================================

/// Two separate strokes, undo one at a time.
#[test]
fn gpu_two_strokes_sequential_undo() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 2 * 1024 * 1024);

    // --- Stroke 1: red circle at (30, 30) ---
    let mut enc = encoder(&device);
    let snap1 = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        30.0,
        30.0,
        5.0,
        [255, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry1 = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &frame(&tex, w, h),
        &snap1,
        cr(23, 23, 14, 14),
    );
    submit(&queue, enc);

    let after_stroke1 = readback_texture(&device, &queue, &tex, fmt, w, h);

    // --- Stroke 2: blue circle at (90, 90) ---
    let mut enc = encoder(&device);
    let snap2 = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        90.0,
        90.0,
        5.0,
        [0, 0, 255, 255],
        1.0,
    );
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry2 = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &frame(&tex, w, h),
        &snap2,
        cr(83, 83, 14, 14),
    );
    submit(&queue, enc);

    // Verify both painted.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 30, 30, 4)[0],
        255,
        "red circle should be present"
    );
    assert_eq!(
        pixel_at(&pixels, w, 90, 90, 4)[2],
        255,
        "blue circle should be present"
    );

    // --- Undo stroke 2 → blue gone, red remains ---
    let mut enc = encoder(&device);
    let forward2 = store.restore_region(&mut enc, &entry2, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 30, 30, 4)[0],
        255,
        "red should remain after undo stroke 2"
    );
    assert_eq!(
        pixel_at(&pixels, w, 90, 90, 4)[3],
        0,
        "blue should be gone after undo stroke 2"
    );

    // Compare with snapshot after stroke 1.
    let [sx, sy, sw, sh] = [23, 23, 14, 14];
    for y in sy..sy + sh {
        for x in sx..sx + sw {
            assert_eq!(
                pixel_at(&pixels, w, x, y, 4),
                pixel_at(&after_stroke1, w, x, y, 4),
                "stroke 1 region should match at ({x},{y})"
            );
        }
    }

    // --- Undo stroke 1 → both gone ---
    let mut enc = encoder(&device);
    let forward1 = store.restore_region(&mut enc, &entry1, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 30, 30, 4)[3],
        0,
        "red should be gone after undo stroke 1"
    );
    assert_eq!(pixel_at(&pixels, w, 90, 90, 4)[3], 0, "blue still gone");

    // --- Redo stroke 1 → red back ---
    let mut enc = encoder(&device);
    let _backward1 = store.restore_region(&mut enc, &forward1, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 30, 30, 4)[0],
        255,
        "red should be back after redo stroke 1"
    );
    assert_eq!(
        pixel_at(&pixels, w, 90, 90, 4)[3],
        0,
        "blue still gone (only redid stroke 1)"
    );

    // --- Redo stroke 2 → blue back ---
    let mut enc = encoder(&device);
    let _backward2 = store.restore_region(&mut enc, &forward2, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 30, 30, 4)[0],
        255,
        "red should still be present"
    );
    assert_eq!(
        pixel_at(&pixels, w, 90, 90, 4)[2],
        255,
        "blue should be back after redo stroke 2"
    );
}

// ============================================================================
// GpuRegionAction + UndoStack integration
// ============================================================================

/// Verify GpuRegionAction integrates with UndoStack (push/pop/complete).
#[test]
fn gpu_region_action_undo_stack() {
    use darkly::document::Document;
    use darkly::undo::{GpuRegionAction, UndoStack};

    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, view) = create_test_texture(&device, &queue, w, h, &red);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);
    let mut undo_stack = UndoStack::new(50);
    let mut doc = Document::new(w, h);

    // save → paint → commit → push.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        32.0,
        32.0,
        8.0,
        [0, 255, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &frame(&tex, w, h),
        &snap,
        cr(22, 22, 20, 20),
    );
    submit(&queue, enc);
    undo_stack.push(&mut doc, Box::new(GpuRegionAction::new(entry)));

    assert!(undo_stack.can_undo());
    assert!(!undo_stack.can_redo());

    // Pop for undo, execute GPU restore, complete.
    let mut action = undo_stack.pop_for_undo().unwrap();
    let affected = action.undo(&mut doc);
    assert!(affected.is_empty(), "GPU action returns empty affected map");

    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let forward = store.restore_region(&mut enc, entry, &frame(&tex, w, h));
        submit(&queue, enc);
        *entry = forward;
    }
    undo_stack.complete_undo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center = pixel_at(&pixels, w, 32, 32, 4);
    assert_eq!(
        center,
        &[255, 0, 0, 255],
        "after undo, should be red, got {:?}",
        center
    );

    assert!(!undo_stack.can_undo());
    assert!(undo_stack.can_redo());

    // Pop for redo, execute GPU restore, complete.
    let mut action = undo_stack.pop_for_redo().unwrap();
    let affected = action.redo(&mut doc);
    assert!(affected.is_empty());

    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let backward = store.restore_region(&mut enc, entry, &frame(&tex, w, h));
        submit(&queue, enc);
        *entry = backward;
    }
    undo_stack.complete_redo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center = pixel_at(&pixels, w, 32, 32, 4);
    assert!(
        center[1] > 0,
        "after redo, green should be visible, got G={}",
        center[1]
    );

    assert!(undo_stack.can_undo());
    assert!(!undo_stack.can_redo());
}

// ============================================================================
// Mixed GPU + CPU undo coexistence
// ============================================================================

/// Push a GPU action, then a CPU PropertyAction, undo both.
#[test]
fn gpu_cpu_undo_interleaved() {
    use darkly::document::Document;
    use darkly::layer::Layer;
    use darkly::undo::property::Property;
    use darkly::undo::{GpuRegionAction, PropertyAction, UndoStack};

    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);
    let mut undo_stack = UndoStack::new(50);
    let mut doc = Document::new(w, h);
    let layer_id = doc.add_raster_layer(None);

    // Step 1: GPU paint stroke.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        32.0,
        32.0,
        5.0,
        [255, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        layer_id,
        &frame(&tex, w, h),
        &snap,
        cr(25, 25, 14, 14),
    );
    submit(&queue, enc);
    undo_stack.push(&mut doc, Box::new(GpuRegionAction::new(entry)));

    // Step 2: CPU property change (opacity).
    undo_stack.push(
        &mut doc,
        Box::new(PropertyAction::new(
            layer_id,
            Property::Opacity(1.0),
            Property::Opacity(0.5),
        )),
    );
    if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
        r.blend.opacity = 0.5;
    }

    // Undo #1: property change (CPU — no GPU work).
    let mut action = undo_stack.pop_for_undo().unwrap();
    let _affected = action.undo(&mut doc);
    assert!(
        action.gpu_region_entry_mut().is_none(),
        "property action should not be GPU"
    );
    undo_stack.complete_undo(action);

    if let Some(Layer::Raster(r)) = doc.layer(layer_id) {
        assert!(
            (r.blend.opacity - 1.0).abs() < f32::EPSILON,
            "opacity should be restored to 1.0"
        );
    }

    // Undo #2: GPU paint stroke.
    let mut action = undo_stack.pop_for_undo().unwrap();
    let _affected = action.undo(&mut doc);
    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let forward = store.restore_region(&mut enc, entry, &frame(&tex, w, h));
        submit(&queue, enc);
        *entry = forward;
    }
    undo_stack.complete_undo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 32, 32, 4)[3],
        0,
        "paint should be undone"
    );

    // Redo both.
    let mut action = undo_stack.pop_for_redo().unwrap();
    let _affected = action.redo(&mut doc);
    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let backward = store.restore_region(&mut enc, entry, &frame(&tex, w, h));
        submit(&queue, enc);
        *entry = backward;
    }
    undo_stack.complete_redo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert!(
        pixel_at(&pixels, w, 32, 32, 4)[0] > 0,
        "paint should be redone"
    );

    let mut action = undo_stack.pop_for_redo().unwrap();
    let _affected = action.redo(&mut doc);
    undo_stack.complete_redo(action);

    if let Some(Layer::Raster(r)) = doc.layer(layer_id) {
        assert!(
            (r.blend.opacity - 0.5).abs() < f32::EPSILON,
            "opacity should be 0.5 after redo"
        );
    }
}

// ============================================================================
// Erase circle via GPU
// ============================================================================

/// Fill red → GPU erase circle → undo → verify red is restored.
#[test]
fn gpu_erase_stroke_undo() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let red: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let (tex, view) = create_test_texture(&device, &queue, w, h, &red);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // begin_stroke: save.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    // stroke_to: erase circle at center.
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.erase_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0);
    submit(&queue, enc);

    // end_stroke: commit.
    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &frame(&tex, w, h),
        &snap,
        cr(52, 52, 24, 24),
    );
    submit(&queue, enc);

    // Verify erased.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 64, 64, 4)[3],
        0,
        "center should be erased"
    );
    assert_eq!(
        pixel_at(&pixels, w, 0, 0, 4)[3],
        255,
        "corner should be unchanged"
    );

    // Undo.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &frame(&tex, w, h));
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 64, 64, 4),
        &[255, 0, 0, 255],
        "center should be restored to red"
    );
}

// ============================================================================
// DiffRectPass: GPU diff-based undo region
// ============================================================================

/// Paint a circle far from the origin, use DiffRectPass to find the changed
/// region, and verify the diff rect covers the painted pixels. This is the
/// mechanism that fixes scatter brush undo — the diff finds the actual changed
/// pixels regardless of where the stroke engine thought they were.
#[test]
fn diff_rect_finds_painted_region() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Two identical transparent textures.
    let blank = vec![0u8; (w * h * 4) as usize];
    let (_scratch_tex, scratch_view) = create_test_texture(&device, &queue, w, h, &blank);
    let (canvas_tex, canvas_view) = create_test_texture(&device, &queue, w, h, &blank);

    // Paint a circle at (100, 100) on the canvas only — simulating a
    // scattered dab that landed far from where the stroke engine tracked.
    let pipelines = PaintPipelines::new(&device, &queue);
    let target = GpuPaintTarget {
        texture: &canvas_tex,
        view: &canvas_view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        100.0,
        100.0,
        8.0,
        [255, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    // Dispatch the diff.
    let mut diff = DiffRectPass::new(&device);
    diff.request(&device, &queue, &scratch_view, &canvas_view, cr(0, 0, w, h));

    // Poll until ready (native/test — blocking poll is fine).
    let rect = loop {
        if let Some(result) = diff.poll(&device) {
            break result;
        }
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    };

    let rect = rect.expect("diff should find changed pixels");
    let (rx, ry, rw, rh) = (rect.x0(), rect.y0(), rect.width, rect.height);

    // The circle is at (100, 100) with radius 8. The diff rect should
    // contain the circle — center must be inside the rect.
    assert!(
        rx <= 100 && 100 < rx + rw as i32,
        "diff rect x range [{}, {}) should contain 100",
        rx,
        rx + rw as i32
    );
    assert!(
        ry <= 100 && 100 < ry + rh as i32,
        "diff rect y range [{}, {}) should contain 100",
        ry,
        ry + rh as i32
    );

    // Rect should be reasonably tight (not the full canvas).
    assert!(rw < 30, "diff rect width should be tight, got {rw}");
    assert!(rh < 30, "diff rect height should be tight, got {rh}");
}

/// Verify that undo fully restores the canvas when the diff rect is used
/// instead of a hand-tracked stroke rect — the key regression test for
/// the scatter brush undo bug.
#[test]
fn diff_rect_undo_restores_offset_paint() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let blank = vec![0u8; (w * h * 4) as usize];
    let (tex, view) = create_test_texture(&device, &queue, w, h, &blank);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // begin_stroke: save full canvas to scratch.
    let mut enc = encoder(&device);
    let snap = store.save_region(&device, &mut enc, &frame(&tex, w, h), fmt, cr(0, 0, w, h));
    submit(&queue, enc);

    // Paint a circle at (100, 100) — far from origin, simulating scatter.
    let target = GpuPaintTarget {
        texture: &tex,
        view: &view,
        format: fmt,
        width: w,
        height: h,
        offset_x: 0,
        offset_y: 0,
        canvas_width: w,
        canvas_height: h,
    };
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        100.0,
        100.0,
        8.0,
        [255, 0, 0, 255],
        1.0,
    );
    submit(&queue, enc);

    // Verify paint landed.
    let painted = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert!(
        pixel_at(&painted, w, 100, 100, 4)[3] > 0,
        "paint should be visible at (100,100)"
    );

    // Compute diff rect via GPU (instead of hand-tracking).
    let scratch_view = store.scratch_view(fmt);
    let mut diff = DiffRectPass::new(&device);
    diff.request(&device, &queue, &scratch_view, &view, cr(0, 0, w, h));

    let rect = loop {
        if let Some(result) = diff.poll(&device) {
            break result;
        }
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    };
    let rect = rect.expect("should have a diff rect");

    // Commit with the diff-derived rect.
    let mut enc = encoder(&device);
    let entry = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &frame(&tex, w, h),
        &snap,
        rect,
    );
    submit(&queue, enc);

    // Undo: restore the pre-stroke state.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &frame(&tex, w, h));
    submit(&queue, enc);

    // The entire canvas should be back to transparent.
    let restored = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&restored, w, 100, 100, 4)[3],
        0,
        "after undo, scattered paint at (100,100) should be gone"
    );

    // Verify the whole canvas is clean.
    for y in 0..h {
        for x in 0..w {
            let a = pixel_at(&restored, w, x, y, 4)[3];
            assert_eq!(
                a, 0,
                "pixel ({x},{y}) should be transparent after undo, got A={a}"
            );
        }
    }
}

/// Regression for canvas-coord snapshot storage: a `Snapshot` saved on a
/// 256×256 layer at canvas (0, 0) survives a negative-direction grow that
/// shifts the layer's local-coord origin by (256, 256). Commit at canvas
/// (-100, -100) must round-trip via `canvas_to_layer_rect` to layer-local
/// (156, 156) in the new frame, and undo must restore the correct pre-stroke
/// pixels at the original canvas position.
#[test]
fn negative_direction_grow_crosses_zero() {
    use wgpu::TextureUsages;
    let (device, queue) = test_device();
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Initial layer: 256×256 at canvas (0, 0), filled red.
    let (init_w, init_h) = (256u32, 256u32);
    let red: Vec<u8> = (0..init_w * init_h)
        .flat_map(|_| [255u8, 0, 0, 255])
        .collect();
    let (initial_tex, _v) = create_test_texture(&device, &queue, init_w, init_h, &red);
    let initial_frame = CanvasFrame {
        texture: &initial_tex,
        canvas_extent: cr(0, 0, init_w, init_h),
    };

    let mut store = RegionStore::with_capacity(&device, init_w, init_h, 4 * 1024 * 1024);

    // Save the full 256×256 layer (canvas (0, 0) → (256, 256)) as the
    // pre-stroke snapshot.
    let mut enc = encoder(&device);
    let mut snap = store.save_region(
        &device,
        &mut enc,
        &initial_frame,
        fmt,
        cr(0, 0, init_w, init_h),
    );
    submit(&queue, enc);

    // Simulate a negative-direction grow: new 512×512 layer at canvas
    // (-256, -256). Old contents land at layer-local (256, 256).
    let (new_w, new_h) = (512u32, 512u32);
    let new_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grown-layer"),
        size: wgpu::Extent3d {
            width: new_w,
            height: new_h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_SRC
            | TextureUsages::COPY_DST
            | TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let mut enc = encoder(&device);
    enc.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &initial_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &new_tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 256,
                y: 256,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: init_w,
            height: init_h,
            depth_or_array_layers: 1,
        },
    );
    // Rebase the region_store scratch alongside the layer.
    store.grow_scratch_preserving(&device, &mut enc, new_w, new_h, 256, 256);
    submit(&queue, enc);

    // After grow the engine widens snap.saved to the new canvas extent so
    // commits that spill into the newly-grown area are still contained.
    snap.saved = cr(-256, -256, new_w, new_h);

    // Frame describing the post-grow layer at canvas (-256, -256).
    let new_frame = CanvasFrame {
        texture: &new_tex,
        canvas_extent: cr(-256, -256, new_w, new_h),
    };

    // Paint blue on top of the (previously red) pre-grow pixels at canvas
    // (50, 50) — translated to new layer-local (306, 306). Then commit a
    // sub-rect that *crosses zero*: canvas (-100, -100, 200, 200) covers
    // both new (transparent) area and the original canvas region.
    let pipelines = PaintPipelines::new(&device, &queue);
    let new_view = new_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let target = GpuPaintTarget {
        texture: &new_tex,
        view: &new_view,
        format: fmt,
        width: new_w,
        height: new_h,
        offset_x: -256,
        offset_y: -256,
        canvas_width: new_w,
        canvas_height: new_h,
    };
    let mut enc = encoder(&device);
    target.composite_circle(
        &mut enc,
        &pipelines,
        &queue,
        50.0,
        50.0,
        20.0,
        [0, 0, 255, 255],
        1.0,
    );
    submit(&queue, enc);

    // Commit a canvas rect that spans (-100, -100) → (100, 100). This is
    // the regression: pre-fix, this rect would not be representable in the
    // pre-grow saved frame and would either panic or write garbage; post-
    // fix, the canvas-coord rect translates cleanly to the new layer's
    // local frame at (156, 156, 200, 200).
    let mut enc = encoder(&device);
    let commit_rect = cr(-100, -100, 200, 200);
    let entry = store.commit_region(
        &mut enc,
        darkly::layer::LayerId::from_ffi(1),
        &new_frame,
        &snap,
        commit_rect,
    );
    submit(&queue, enc);

    // Undo: restore the pre-stroke pixels at canvas (-100, -100, 200, 200).
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &new_frame);
    submit(&queue, enc);

    // After undo, canvas (50, 50) should be back to red (the pre-stroke
    // state), and canvas (-100, -100) (which was zeroed pre-grow) should
    // be transparent — both correctly restored.
    let pixels = readback_texture(&device, &queue, &new_tex, fmt, new_w, new_h);
    // canvas (50, 50) → layer-local (306, 306).
    let p = pixel_at(&pixels, new_w, 306, 306, 4);
    assert_eq!(
        p,
        &[255, 0, 0, 255],
        "canvas (50, 50) should be the pre-stroke red after undo, got {:?}",
        p
    );
    // canvas (-100, -100) → layer-local (156, 156). Should be transparent.
    let q = pixel_at(&pixels, new_w, 156, 156, 4);
    assert_eq!(
        q[3], 0,
        "canvas (-100, -100) should be transparent after undo, got A={}",
        q[3]
    );
}
