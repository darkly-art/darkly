//! Headless profiling test for the compositor render pipeline.
//!
//! Run with: cargo test -p darkly --test profile_render -- --nocapture
//!
//! This exercises the full pipeline (paint → dirty → upload → composite → submit)
//! on native wgpu (software backend) and prints per-phase timing.

use darkly::document::Document;
use darkly::gpu::compositor::Compositor;
use std::time::Instant;

/// Request a headless wgpu device (no window, no surface).
fn headless_device() -> (wgpu::Device, wgpu::Queue) {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .expect("No suitable GPU adapter found (not even software fallback)");

        eprintln!("adapter: {:?}", adapter.get_info());

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("darkly-test-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .expect("Failed to create device");

        (device, queue)
    })
}

struct FrameTiming {
    paint_us: u128,
    render_us: u128,
    submitted: bool,
}

fn run_paint_benchmark(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    compositor: &mut Compositor,
    doc: &mut Document,
    paint_layer_id: u64,
    num_frames: usize,
) -> Vec<FrameTiming> {
    let mut timings = Vec::with_capacity(num_frames);

    for i in 0..num_frames {
        // Simulate a brush dab at varying positions (256px radius = 512x512 dab)
        let x = 200.0 + ((i as f32) * 2.0) % 1500.0;
        let y = 300.0 + ((i as f32) * 0.7).sin() * 20.0;

        let t_paint = Instant::now();
        doc.paint_circle(paint_layer_id, x, y, 256.0, [220, 180, 60, 200]);
        let paint_us = t_paint.elapsed().as_micros();

        let t_render = Instant::now();
        let submitted = compositor.render_offscreen(device, queue, doc);
        let render_us = t_render.elapsed().as_micros();

        timings.push(FrameTiming {
            paint_us,
            render_us,
            submitted,
        });
    }

    timings
}

#[test]
fn profile_render_pipeline() {
    let (device, queue) = headless_device();

    let width = 1920u32;
    let height = 1080u32;
    // Use Rgba8Unorm as the "surface format" for the compositor.
    // In the browser this would be an sRGB surface, but for profiling the
    // compositing pipeline the format of the present pipeline doesn't matter —
    // we never call render() with a surface.
    let surface_format = wgpu::TextureFormat::Rgba8Unorm;

    let mut compositor = Compositor::new(&device, &queue, surface_format, width, height, false);
    let mut doc = Document::new(width, height);

    // Set up layers matching App.svelte: gradient bg + noise filter + paint layer
    let bg_id = doc.add_raster_layer();
    compositor.ensure_raster_layer(&device, &queue, bg_id);
    doc.fill_gradient(bg_id);

    let paint_id = doc.add_raster_layer();
    compositor.ensure_raster_layer(&device, &queue, paint_id);

    // Warm up: first render composites everything (full canvas)
    let t_warmup = Instant::now();
    let _ = compositor.render_offscreen(&device, &queue, &mut doc);
    let warmup_us = t_warmup.elapsed().as_micros();
    eprintln!("warmup (full composite): {:.1}ms", warmup_us as f64 / 1000.0);

    // Benchmark: 10000 frames of brush dabs on the top paint layer
    let num_frames = 10_000;
    let timings = run_paint_benchmark(
        &device,
        &queue,
        &mut compositor,
        &mut doc,
        paint_id,
        num_frames,
    );

    // Compute stats
    let paint_times: Vec<f64> = timings.iter().map(|t| t.paint_us as f64 / 1000.0).collect();
    let render_times: Vec<f64> = timings
        .iter()
        .filter(|t| t.submitted)
        .map(|t| t.render_us as f64 / 1000.0)
        .collect();
    let skipped = timings.iter().filter(|t| !t.submitted).count();

    let paint_avg = paint_times.iter().sum::<f64>() / paint_times.len() as f64;
    let paint_max = paint_times.iter().cloned().fold(0.0f64, f64::max);

    let render_avg = if render_times.is_empty() {
        0.0
    } else {
        render_times.iter().sum::<f64>() / render_times.len() as f64
    };
    let render_max = render_times.iter().cloned().fold(0.0f64, f64::max);
    let render_p50 = percentile(&render_times, 50.0);
    let render_p95 = percentile(&render_times, 95.0);
    let render_p99 = percentile(&render_times, 99.0);

    eprintln!();
    eprintln!("=== PROFILE: {num_frames} frames, {width}x{height} canvas, 3 layers ===");
    eprintln!("paint_circle:  avg={paint_avg:.2}ms  max={paint_max:.2}ms");
    eprintln!(
        "render:        avg={render_avg:.2}ms  p50={render_p50:.2}ms  p95={render_p95:.2}ms  p99={render_p99:.2}ms  max={render_max:.2}ms"
    );
    eprintln!("skipped (no-op): {skipped}/{num_frames}");
    eprintln!();

    // Print per-frame detail for first 10 + last 5
    eprintln!("frame  paint_ms  render_ms  submitted");
    for (i, t) in timings.iter().enumerate() {
        if i < 10 || i >= num_frames - 5 {
            eprintln!(
                "{:>5}  {:>8.2}  {:>9.2}  {}",
                i,
                t.paint_us as f64 / 1000.0,
                t.render_us as f64 / 1000.0,
                t.submitted,
            );
        } else if i == 10 {
            eprintln!("  ...");
        }
    }
}

fn percentile(sorted_data: &[f64], p: f64) -> f64 {
    if sorted_data.is_empty() {
        return 0.0;
    }
    let mut data = sorted_data.to_vec();
    data.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((p / 100.0) * (data.len() - 1) as f64).round() as usize;
    data[idx.min(data.len() - 1)]
}
