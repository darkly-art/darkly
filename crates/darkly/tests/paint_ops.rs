//! Paint operation GPU integration tests: gradient, flood fill, color picker, fill rect.
//!
//! Tests the GPU paint operations that don't involve selection masking.
//! Run with: `cargo test -p darkly --test paint_ops`

use darkly::gpu::paint_target::{GpuPaintTarget, PaintPipelines};
use darkly::gpu::region_store::RegionStore;
use darkly::gpu::test_utils::*;

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
// GPU Gradient
// ============================================================================

/// Render a gradient from white (top-left) to black (bottom-right).
/// Verify pixel values follow linear interpolation.
#[test]
fn gpu_gradient_linear_interpolation() {
    let (device, queue) = test_device();
    let (w, h) = (128, 128);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);

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
    target.linear_gradient(
        &mut enc,
        &pipelines,
        &queue,
        0.0,
        0.0, // start: top-left
        128.0,
        128.0,                // end: bottom-right
        [255, 255, 255, 255], // white
        [0, 0, 0, 255],       // black
        None,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Top-left corner should be white (or near-white).
    let tl = pixel_at(&pixels, w, 0, 0, 4);
    assert!(tl[0] > 200, "top-left R should be near 255, got {}", tl[0]);
    assert_eq!(tl[3], 255, "alpha should be 255");

    // Bottom-right corner should be black (or near-black).
    let br = pixel_at(&pixels, w, 127, 127, 4);
    assert!(br[0] < 55, "bottom-right R should be near 0, got {}", br[0]);
    assert_eq!(br[3], 255, "alpha should be 255");

    // Center should be roughly midpoint.
    let mid = pixel_at(&pixels, w, 64, 64, 4);
    assert!(
        mid[0] > 80 && mid[0] < 175,
        "center R should be roughly 128, got {}",
        mid[0]
    );
}

/// Gradient with undo: render gradient → undo → verify layer is blank.
#[test]
fn gpu_gradient_undo() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 1024 * 1024);

    // Save pre-gradient state.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Render gradient.
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
    target.linear_gradient(
        &mut enc,
        &pipelines,
        &queue,
        0.0,
        0.0,
        64.0,
        0.0,
        [255, 0, 0, 255],
        [0, 0, 255, 255],
        None,
    );
    submit(&queue, enc);

    // Commit for undo.
    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Verify gradient was painted.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert!(pixel_at(&pixels, w, 0, 0, 4)[0] > 200, "left should be red");
    assert!(
        pixel_at(&pixels, w, 63, 0, 4)[2] > 200,
        "right should be blue"
    );

    // Undo.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 0, 0, 4)[3],
        0,
        "after undo, should be transparent"
    );
    assert_eq!(
        pixel_at(&pixels, w, 63, 0, 4)[3],
        0,
        "after undo, should be transparent"
    );
}

// ============================================================================
// GPU Flood Fill (hybrid)
// ============================================================================

/// Paint a closed red rectangle, flood fill interior with blue, verify.
#[test]
fn gpu_flood_fill_interior() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Start with transparent canvas.
    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);

    // Paint a red border rectangle (10,10)-(50,50) by filling 4 sides.
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

    // Use fill_rect to paint 4 border strips.
    let red = [255u8, 0, 0, 255];
    let mut enc = encoder(&device);
    target.fill_rect(&mut enc, &pipelines, &queue, [10, 10, 40, 2], red); // top
    submit(&queue, enc);
    let mut enc = encoder(&device);
    target.fill_rect(&mut enc, &pipelines, &queue, [10, 48, 40, 2], red); // bottom
    submit(&queue, enc);
    let mut enc = encoder(&device);
    target.fill_rect(&mut enc, &pipelines, &queue, [10, 10, 2, 40], red); // left
    submit(&queue, enc);
    let mut enc = encoder(&device);
    target.fill_rect(&mut enc, &pipelines, &queue, [48, 10, 2, 40], red); // right
    submit(&queue, enc);

    // Readback to get the pixel data for CPU flood fill.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Flood fill from interior point (30, 30) with blue.
    let fill_mask = darkly::gpu::flood_fill::flood_fill_rgba(&pixels, w, h, 30, 30, 0);

    // Verify the fill mask covers the interior.
    assert_eq!(
        fill_mask[(30 * w + 30) as usize],
        255,
        "interior should be filled"
    );
    assert_eq!(fill_mask[0], 0, "exterior should not be filled");
    // Border pixel should not be filled (it's red, not transparent).
    assert_eq!(
        fill_mask[(10 * w + 10) as usize],
        0,
        "border should not be filled"
    );

    // Upload mask and stamp.
    let mask_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-fill-mask"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &mask_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &fill_mask,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let mask_view = mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let mask_bg = pipelines.create_selection_bind_group(&device, &mask_view, &sampler);

    let mut enc = encoder(&device);
    target.fill_rect_with_selection(
        &mut enc,
        &pipelines,
        &queue,
        [0, 0, w, h],
        [0, 0, 255, 255],
        &mask_bg,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Interior should be blue.
    let interior = pixel_at(&pixels, w, 30, 30, 4);
    assert!(
        interior[2] > 200,
        "interior should be blue, B={}",
        interior[2]
    );
    assert!(
        interior[0] < 55,
        "interior R should be low, got {}",
        interior[0]
    );

    // Border should still be red.
    let border = pixel_at(&pixels, w, 10, 10, 4);
    assert_eq!(border[0], 255, "border should be red");

    // Exterior should be transparent.
    let exterior = pixel_at(&pixels, w, 0, 0, 4);
    assert_eq!(exterior[3], 0, "exterior should be transparent");
}

/// Flood fill with undo round-trip.
#[test]
fn gpu_flood_fill_undo() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);
    let mut store = RegionStore::with_capacity(&device, w, h, 2 * 1024 * 1024);

    // Save region for undo.
    let mut enc = encoder(&device);
    store.save_region(&mut enc, &tex, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Flood fill entire canvas (all transparent → seed matches everywhere).
    let pixels_before = readback_texture(&device, &queue, &tex, fmt, w, h);
    let fill_mask = darkly::gpu::flood_fill::flood_fill_rgba(&pixels_before, w, h, 0, 0, 0);

    let mask_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-fill-mask"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &mask_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &fill_mask,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let mask_view = mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let mask_bg = pipelines.create_selection_bind_group(&device, &mask_view, &sampler);

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
    target.fill_rect_with_selection(
        &mut enc,
        &pipelines,
        &queue,
        [0, 0, w, h],
        [0, 255, 0, 255],
        &mask_bg,
    );
    submit(&queue, enc);

    // Commit undo entry.
    let mut enc = encoder(&device);
    let entry = store.commit_region(&mut enc, 1, fmt, [0, 0, w, h]);
    submit(&queue, enc);

    // Verify fill landed.
    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert!(
        pixel_at(&pixels, w, 32, 32, 4)[1] > 200,
        "should be green after fill"
    );

    // Undo.
    let mut enc = encoder(&device);
    let _forward = store.restore_region(&mut enc, &entry, &tex);
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);
    assert_eq!(
        pixel_at(&pixels, w, 32, 32, 4)[3],
        0,
        "after undo, should be transparent"
    );
}

// ============================================================================
// Color Picker (readback single pixel)
// ============================================================================

/// Read back individual pixels from a texture with known colors.
#[test]
fn gpu_color_pick_readback() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // Paint red at (10,10), blue at (50,50), rest transparent.
    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);

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
    target.fill_rect(
        &mut enc,
        &pipelines,
        &queue,
        [10, 10, 1, 1],
        [255, 0, 0, 255],
    );
    submit(&queue, enc);

    let mut enc = encoder(&device);
    target.fill_rect(
        &mut enc,
        &pipelines,
        &queue,
        [50, 50, 1, 1],
        [0, 0, 255, 255],
    );
    submit(&queue, enc);

    // Read back individual pixels.
    let pick = |x: u32, y: u32| -> [u8; 4] {
        let mut enc = encoder(&device);
        let request =
            darkly::gpu::readback::request_readback(&device, &mut enc, &tex, fmt, [x, y, 1, 1]);
        submit(&queue, enc);
        let data = request.blocking_read(&device);
        [data[0], data[1], data[2], data[3]]
    };

    let red = pick(10, 10);
    assert_eq!(
        red,
        [255, 0, 0, 255],
        "should pick red at (10,10), got {:?}",
        red
    );

    let blue = pick(50, 50);
    assert_eq!(
        blue,
        [0, 0, 255, 255],
        "should pick blue at (50,50), got {:?}",
        blue
    );

    let empty = pick(0, 0);
    assert_eq!(
        empty,
        [0, 0, 0, 0],
        "should pick transparent at (0,0), got {:?}",
        empty
    );
}

// ============================================================================
// Gradient on R8 mask target
// ============================================================================

/// Render gradient on an R8 mask texture.
#[test]
fn gpu_gradient_on_mask() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::R8Unorm;

    // Start with fully-revealed mask (255).
    let white = vec![255u8; (w * h) as usize];
    let (tex, view) = create_test_texture_with_format(&device, &queue, w, h, &white, fmt);
    let pipelines = PaintPipelines::new(&device, &queue);

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

    // Gradient from white to black (left to right).
    let mut enc = encoder(&device);
    target.linear_gradient(
        &mut enc,
        &pipelines,
        &queue,
        0.0,
        0.0,
        64.0,
        0.0,
        [255, 255, 255, 255], // luminance = 1.0
        [0, 0, 0, 255],       // luminance = 0.0
        None,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Left side should be bright (near 255).
    assert!(
        pixels[(32 * w + 2) as usize] > 200,
        "left should be bright, got {}",
        pixels[(32 * w + 2) as usize]
    );

    // Right side should be dark (near 0).
    assert!(
        pixels[(32 * w + 62) as usize] < 55,
        "right should be dark, got {}",
        pixels[(32 * w + 62) as usize]
    );
}

// ============================================================================
// Fill rect with selection (used by flood fill stamp)
// ============================================================================

/// Fill rect with a custom mask — verify masked and unmasked regions.
#[test]
fn gpu_fill_rect_with_mask() {
    let (device, queue) = test_device();
    let (w, h) = (64, 64);
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    let (tex, view) = create_test_texture(&device, &queue, w, h, &vec![0u8; (w * h * 4) as usize]);
    let pipelines = PaintPipelines::new(&device, &queue);

    // Mask: circle in center (simple: 32×32 block at center).
    let mut mask_data = vec![0u8; (w * h) as usize];
    for y in 16..48 {
        for x in 16..48 {
            mask_data[(y * w + x) as usize] = 255;
        }
    }
    let (mask_tex, _) = create_test_texture_with_format(
        &device,
        &queue,
        w,
        h,
        &mask_data,
        wgpu::TextureFormat::R8Unorm,
    );
    let mask_view = mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let mask_bg = pipelines.create_selection_bind_group(&device, &mask_view, &sampler);

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
    target.fill_rect_with_selection(
        &mut enc,
        &pipelines,
        &queue,
        [0, 0, w, h],
        [0, 255, 0, 255],
        &mask_bg,
    );
    submit(&queue, enc);

    let pixels = readback_texture(&device, &queue, &tex, fmt, w, h);

    // Inside mask: should be green.
    let inside = pixel_at(&pixels, w, 32, 32, 4);
    assert_eq!(
        inside[1], 255,
        "inside mask should be green, G={}",
        inside[1]
    );
    assert_eq!(inside[3], 255, "inside mask alpha should be 255");

    // Outside mask: should be transparent.
    let outside = pixel_at(&pixels, w, 0, 0, 4);
    assert_eq!(
        outside[3], 0,
        "outside mask should be transparent, A={}",
        outside[3]
    );
}
