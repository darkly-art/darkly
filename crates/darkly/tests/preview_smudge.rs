//! Hover-cursor preview render through `smudge`.
//! The stroke body samples the scratch read mirror (bound at
//! `@group(3)` in stroke mode); preview mode omits that group, so
//! `smudge` overrides `compile_preview_body` to emit a
//! neutral-gray-modulated-by-mask body. Verifies the override fires
//! and the resulting preview shows the brush footprint in gray.

use std::sync::Arc;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, PreviewState};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::gpu::test_utils::{readback_texture, test_device};

const PREVIEW_SIDE: u32 = 256;

fn preview_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("preview-target"),
        size: wgpu::Extent3d {
            width: PREVIEW_SIDE,
            height: PREVIEW_SIDE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * PREVIEW_SIDE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn smudge_preview_shows_neutral_gray_footprint() {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Smudge")
        .expect("Smudge brush registered");
    let mut graph = brush.metadata.graph.clone();
    let term_id = darkly::brush::find_terminal(&graph).expect("brush has a terminal");
    graph.set_port_default(term_id, "size", 0.1).unwrap();

    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(&device, &queue);
    let (target_tex, target_view) = preview_target(&device);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("preview-smudge"),
    });
    let mut ctx = BrushGpuContext {
        encoder,
        device: &device,
        queue: &queue,
        pipelines: &pipelines,
        selection_bind_group: pipelines.default_selection_bind_group(),
        canvas_width: PREVIEW_SIDE,
        canvas_height: PREVIEW_SIDE,
        blend_mode: 0,
        perf: BrushPerfCounters::default(),
        stroke: None,
        preview: Some(PreviewState {
            mask_view: Some(&target_view),
            mask_size: (PREVIEW_SIDE, PREVIEW_SIDE),
            mask_overlay: None,
            info: None,
        }),
        dab_batch: DabBatch::default(),
    };

    let info = PaintInformation {
        pos: [PREVIEW_SIDE as f32 * 0.5, PREVIEW_SIDE as f32 * 0.5],
        pressure: 1.0,
        ..Default::default()
    };
    runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0xC0FFEE, 0);
    runner.execute_cpu();
    runner.render_preview_pipeline(&mut ctx);
    let published = ctx
        .preview
        .as_ref()
        .and_then(|p| p.info)
        .expect("smudge publishes brush_preview_info");
    queue.submit([ctx.encoder.finish()]);

    let rgba = readback_texture(
        &device,
        &queue,
        &target_tex,
        wgpu::TextureFormat::Rgba8Unorm,
        PREVIEW_SIDE,
        PREVIEW_SIDE,
    );

    let half = PREVIEW_SIDE / 2;
    let centre = px(&rgba, half, half);
    // The override emits `vec3<f32>(0.6, 0.6, 0.6) * mask` (with
    // premultiplied alpha = mask). At centre with a non-degenerate
    // shape, mask ≈ 1.0 → centre ≈ (153, 153, 153, 255). Allow
    // wiggle for softness falloff.
    assert!(
        centre[3] > 100,
        "Smudge preview centre should have meaningful alpha; got {centre:?}",
    );
    // Neutral gray: R ≈ G ≈ B (the override doesn't read brush color).
    assert!(
        (centre[0] as i32 - centre[1] as i32).abs() < 5
            && (centre[1] as i32 - centre[2] as i32).abs() < 5,
        "Smudge preview should be neutral gray; got {centre:?}",
    );
    // Smudge cursor is symmetric — rotation_rad is fixed at 0.0.
    assert_eq!(published.rotation_rad, 0.0);
}
