//! Brush stroke GPU integration tests: stroke workflows, undo/redo, erase.
//!
//! Tests the end-to-end GPU brush flow using raw components (RegionStore,
//! GpuPaintTarget, GpuRegionAction) without a full DarklyEngine.
//! Run with: `cargo test -p darkly --test stroke`

use darkly::gpu::test_utils::*;
use darkly::gpu::region_store::RegionStore;
use darkly::gpu::paint_target::{GpuPaintTarget, PaintPipelines};

fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") })
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
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // --- stroke_to: two circles ---
    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };

    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 50.0, 50.0, 5.0, [255, 0, 0, 255], 1.0);
    submit(&queue, enc);

    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 60.0, 50.0, 5.0, [255, 0, 0, 255], 1.0);
    submit(&queue, enc);

    // --- end_stroke: commit the stroke rect ---
    // Bounding rect: x=45..65, y=45..55 (approx) — use conservative rect.
    let stroke_rect = [43, 43, 24, 14];
    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, stroke_rect);
    submit(&queue, enc);

    // Verify paint landed.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center1 = pixel_at(&pixels, w, 50, 50, 4);
    assert_eq!(center1[0], 255, "circle 1 center should be red, R={}", center1[0]);
    assert_eq!(center1[3], 255, "circle 1 center should be opaque, A={}", center1[3]);

    let center2 = pixel_at(&pixels, w, 60, 50, 4);
    assert!(center2[3] > 0, "circle 2 center should be visible, A={}", center2[3]);

    let painted_snapshot = pixels.clone();

    // --- undo ---
    let mut enc = encoder(&device);
    let forward_entry = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center1 = pixel_at(&pixels, w, 50, 50, 4);
    assert_eq!(center1[3], 0, "after undo, circle 1 should be gone, A={}", center1[3]);

    let corner = pixel_at(&pixels, w, 0, 0, 4);
    assert_eq!(corner[3], 0, "corner should still be transparent");

    // --- redo ---
    let mut enc = encoder(&device);
    let _backward_entry = store.restore_region(&mut enc, &forward_entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    // The stroke rect area should be pixel-identical to the painted snapshot.
    let [sx, sy, sw, sh] = stroke_rect;
    for y in sy..sy + sh {
        for x in sx..sx + sw {
            let p = pixel_at(&pixels, w, x, y, 4);
            let expected = pixel_at(&painted_snapshot, w, x, y, 4);
            assert_eq!(p, expected, "redo mismatch at ({x},{y}): got {:?}, expected {:?}", p, expected);
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
        cx: f32, cy: f32, radius: f32,
        canvas_w: u32, canvas_h: u32,
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
    assert!(rect[0] + rect[2] >= 112, "x1 should be ≥ 112, got {}", rect[0] + rect[2]);
    assert!(rect[1] + rect[3] >= 112, "y1 should be ≥ 112, got {}", rect[1] + rect[3]);

    // Second circle at (200, 200, r=10) — rect should expand.
    let rect = expand_rect(Some(rect), 200.0, 200.0, 10.0, canvas.0, canvas.1);
    assert!(rect[0] <= 88, "expanded x0 should still cover first circle");
    assert!(rect[0] + rect[2] >= 212, "expanded x1 should cover second circle");
    assert!(rect[1] + rect[3] >= 212, "expanded y1 should cover second circle");

    // Circle near edge (0, 0, r=5) — clamped to canvas bounds.
    let rect = expand_rect(None, 0.0, 0.0, 5.0, canvas.0, canvas.1);
    assert_eq!(rect[0], 0, "clamped x0 should be 0");
    assert_eq!(rect[1], 0, "clamped y0 should be 0");

    // Small stroke rect should be << canvas size.
    let small = expand_rect(None, 128.0, 128.0, 3.0, canvas.0, canvas.1);
    assert!(small[2] < 20, "small stroke width should be < 20, got {}", small[2]);
    assert!(small[3] < 20, "small stroke height should be < 20, got {}", small[3]);
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
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // stroke_to: paint black circles → mask toward 0.
    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };

    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0, [0, 0, 0, 255], 1.0);
    submit(&queue, enc);

    // end_stroke: commit.
    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [52, 52, 24, 24]);
    submit(&queue, enc);

    // Verify mask is painted.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center = (64 * w + 64) as usize;
    assert!(pixels[center] < 10, "mask center should be near 0, got {}", pixels[center]);
    assert_eq!(pixels[0], 255, "mask corner should be 255");

    // Undo: restore mask to 255.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixels[center], 255, "after undo, mask center should be 255, got {}", pixels[center]);
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
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };
    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 30.0, 30.0, 5.0, [255, 0, 0, 255], 1.0);
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry1 = store.commit_region(&mut enc, 1, fmt, [23, 23, 14, 14]);
    submit(&queue, enc);

    let after_stroke1 = readback_texture(&device, &queue, &tex, fmt, w, h);

    // --- Stroke 2: blue circle at (90, 90) ---
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };
    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 90.0, 90.0, 5.0, [0, 0, 255, 255], 1.0);
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry2 = store.commit_region(&mut enc, 1, fmt, [83, 83, 14, 14]);
    submit(&queue, enc);

    // Verify both painted.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 30, 30, 4)[0], 255, "red circle should be present");
    assert_eq!(pixel_at(&pixels, w, 90, 90, 4)[2], 255, "blue circle should be present");

    // --- Undo stroke 2 → blue gone, red remains ---
    let mut enc = encoder(&device);
    let forward2 = store.restore_region(&mut enc, &entry2, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 30, 30, 4)[0], 255, "red should remain after undo stroke 2");
    assert_eq!(pixel_at(&pixels, w, 90, 90, 4)[3], 0, "blue should be gone after undo stroke 2");

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
    let forward1 = store.restore_region(&mut enc, &entry1, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 30, 30, 4)[3], 0, "red should be gone after undo stroke 1");
    assert_eq!(pixel_at(&pixels, w, 90, 90, 4)[3], 0, "blue still gone");

    // --- Redo stroke 1 → red back ---
    let mut enc = encoder(&device);
    let _backward1 = store.restore_region(&mut enc, &forward1, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 30, 30, 4)[0], 255, "red should be back after redo stroke 1");
    assert_eq!(pixel_at(&pixels, w, 90, 90, 4)[3], 0, "blue still gone (only redid stroke 1)");

    // --- Redo stroke 2 → blue back ---
    let mut enc = encoder(&device);
    let _backward2 = store.restore_region(&mut enc, &forward2, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 30, 30, 4)[0], 255, "red should still be present");
    assert_eq!(pixel_at(&pixels, w, 90, 90, 4)[2], 255, "blue should be back after redo stroke 2");
}

// ============================================================================
// GpuRegionAction + UndoStack integration
// ============================================================================

/// Verify GpuRegionAction integrates with UndoStack (push/pop/complete).
#[test]
fn gpu_region_action_undo_stack() {
    use darkly::document::Document;
    use darkly::undo::{UndoStack, GpuRegionAction};

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
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };
    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 32.0, 32.0, 8.0, [0, 255, 0, 255], 1.0);
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [22, 22, 20, 20]);
    submit(&queue, enc);
    undo_stack.push(Box::new(GpuRegionAction::new(entry)));

    assert!(undo_stack.can_undo());
    assert!(!undo_stack.can_redo());

    // Pop for undo, execute GPU restore, complete.
    let mut action = undo_stack.pop_for_undo().unwrap();
    let affected = action.undo(&mut doc);
    assert!(affected.is_empty(), "GPU action returns empty affected map");

    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let forward = store.restore_region(&mut enc, entry, &tex);
        submit(&queue, enc);
        *entry = forward;
    }
    undo_stack.complete_undo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center = pixel_at(&pixels, w, 32, 32, 4);
    assert_eq!(center, &[255, 0, 0, 255], "after undo, should be red, got {:?}", center);

    assert!(!undo_stack.can_undo());
    assert!(undo_stack.can_redo());

    // Pop for redo, execute GPU restore, complete.
    let mut action = undo_stack.pop_for_redo().unwrap();
    let affected = action.redo(&mut doc);
    assert!(affected.is_empty());

    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let backward = store.restore_region(&mut enc, entry, &tex);
        submit(&queue, enc);
        *entry = backward;
    }
    undo_stack.complete_redo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    let center = pixel_at(&pixels, w, 32, 32, 4);
    assert!(center[1] > 0, "after redo, green should be visible, got G={}", center[1]);

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
    use darkly::undo::{UndoStack, GpuRegionAction, PropertyAction};
    use darkly::undo::property::Property;

    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);
    let mut undo_stack = UndoStack::new(50);
    let mut doc = Document::new(w, h);
    let layer_id = doc.add_raster_layer();

    // Step 1: GPU paint stroke.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };
    let mut enc = encoder(&device);
    target.composite_circle(&mut enc, &pipelines, &queue, 32.0, 32.0, 5.0, [255, 0, 0, 255], 1.0);
    submit(&queue, enc);

    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, layer_id, fmt, [25, 25, 14, 14]);
    submit(&queue, enc);
    undo_stack.push(Box::new(GpuRegionAction::new(entry)));

    // Step 2: CPU property change (opacity).
    undo_stack.push(Box::new(PropertyAction::new(
        layer_id,
        Property::Opacity(1.0),
        Property::Opacity(0.5),
    )));
    if let Some(Layer::Raster(r)) = doc.layer_mut(layer_id) {
        r.opacity = 0.5;
    }

    // Undo #1: property change (CPU — no GPU work).
    let mut action = undo_stack.pop_for_undo().unwrap();
    let _affected = action.undo(&mut doc);
    assert!(action.gpu_region_entry_mut().is_none(), "property action should not be GPU");
    undo_stack.complete_undo(action);

    if let Some(Layer::Raster(r)) = doc.layer(layer_id) {
        assert!((r.opacity - 1.0).abs() < f32::EPSILON, "opacity should be restored to 1.0");
    }

    // Undo #2: GPU paint stroke.
    let mut action = undo_stack.pop_for_undo().unwrap();
    let _affected = action.undo(&mut doc);
    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let forward = store.restore_region(&mut enc, entry, &tex);
        submit(&queue, enc);
        *entry = forward;
    }
    undo_stack.complete_undo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 32, 32, 4)[3], 0, "paint should be undone");

    // Redo both.
    let mut action = undo_stack.pop_for_redo().unwrap();
    let _affected = action.redo(&mut doc);
    if let Some(entry) = action.gpu_region_entry_mut() {
        let mut enc = encoder(&device);
        let backward = store.restore_region(&mut enc, entry, &tex);
        submit(&queue, enc);
        *entry = backward;
    }
    undo_stack.complete_redo(action);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert!(pixel_at(&pixels, w, 32, 32, 4)[0] > 0, "paint should be redone");

    let mut action = undo_stack.pop_for_redo().unwrap();
    let _affected = action.redo(&mut doc);
    undo_stack.complete_redo(action);

    if let Some(Layer::Raster(r)) = doc.layer(layer_id) {
        assert!((r.opacity - 0.5).abs() < f32::EPSILON, "opacity should be 0.5 after redo");
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
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // stroke_to: erase circle at center.
    let target = GpuPaintTarget { texture: &tex, view: &view, format: fmt, width: w, height: h };
    let mut enc = encoder(&device);
    target.erase_circle(&mut enc, &pipelines, &queue, 64.0, 64.0, 10.0);
    submit(&queue, enc);

    // end_stroke: commit.
    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [52, 52, 24, 24]);
    submit(&queue, enc);

    // Verify erased.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 64, 64, 4)[3], 0, "center should be erased");
    assert_eq!(pixel_at(&pixels, w, 0, 0, 4)[3], 255, "corner should be unchanged");

    // Undo.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(pixel_at(&pixels, w, 64, 64, 4), &[255, 0, 0, 255], "center should be restored to red");
}
