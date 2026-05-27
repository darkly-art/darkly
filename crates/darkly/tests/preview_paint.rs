//! Hover-cursor preview render through `paint`. Verifies the
//! shared `render_compiled_preview` helper produces the brush's
//! actual color × shape × flow into the preview mask, and publishes
//! sane placement info via `BrushPreviewInfo`.
//!
//! Runs against the live built-in brush graphs (no test-only
//! rewiring) so per-brush wire bugs surface here.

use std::sync::Arc;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
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

struct PreviewOutput {
    rgba: Vec<u8>,
    info: darkly::brush::eval::BrushPreviewInfo,
}

fn render_preview(brush_name: &str, size_override: f32, color: [f32; 4]) -> PreviewOutput {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == brush_name)
        .unwrap_or_else(|| panic!("builtin brush `{brush_name}` not registered"));
    let mut graph = brush.metadata.graph.clone();
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "paint")
        .map(|(id, _)| *id)
        .expect("brush terminates in paint");
    graph
        .set_port_default(term_id, "size", size_override)
        .unwrap();

    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(&device, &queue);
    let (target_tex, target_view) = preview_target(&device);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");

    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("preview-paint"),
    });
    let mut ctx = BrushGpuContext {
        encoder,
        device: &device,
        queue: &queue,
        pipelines: &pipelines,
        scratch: None,
        canvas_width: PREVIEW_SIDE,
        canvas_height: PREVIEW_SIDE,
        paint_target: None,
        selection_bind_group: pipelines.default_selection_bind_group(),
        preview_target_view: Some(&target_view),
        blend_mode: 0,
        preview_mask_view: Some(&target_view),
        preview_mask_size: (PREVIEW_SIDE, PREVIEW_SIDE),
        preview_mask_overlay: None,
        brush_preview_info: None,
        pre_stroke_texture: None,
        pre_stroke_bind_group: None,
        dab_write_canvas_bbox: None,
        perf: BrushPerfCounters::default(),
        pending_dab_bytes: Vec::new(),
        pending_dab_count: 0,
        pending_dabs_bbox: None,
        pending_dab_meta_bytes: Vec::new(),
        compiled_brush: None,
        slot_outputs_owned: None,
    };

    let info = PaintInformation {
        pos: [PREVIEW_SIDE as f32 * 0.5, PREVIEW_SIDE as f32 * 0.5],
        pressure: 1.0,
        ..Default::default()
    };
    runner.seed_sensors(&info, color, 0xC0FFEE, 0);
    runner.execute_cpu();
    runner.render_preview_pipeline(&mut ctx);
    let published = ctx
        .brush_preview_info
        .expect("paint publishes brush_preview_info");
    queue.submit([ctx.encoder.finish()]);

    let rgba = readback_texture(
        &device,
        &queue,
        &target_tex,
        wgpu::TextureFormat::Rgba8Unorm,
        PREVIEW_SIDE,
        PREVIEW_SIDE,
    );
    PreviewOutput {
        rgba,
        info: published,
    }
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * PREVIEW_SIDE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn round_brush_preview_shows_color_and_shape() {
    // Round: pressure=1 → flow=1 → centre pixel must be the seeded
    // color at near-full alpha.
    let out = render_preview("Round", 0.1, [1.0, 0.0, 0.0, 1.0]);
    let half = PREVIEW_SIDE / 2;
    let centre = px(&out.rgba, half, half);
    assert!(
        centre[0] > 200 && centre[1] < 30 && centre[2] < 30 && centre[3] > 200,
        "Round centre should be opaque red; got {centre:?}",
    );

    // Sanity-check bbox info published.
    assert!(out.info.half_extent_canvas_px[0] > 0.0);
    assert!(out.info.half_extent_canvas_px[1] > 0.0);

    // Outside the disc footprint, the mask discards → fully
    // transparent. Sample a corner well beyond the brush extent.
    let corner = px(&out.rgba, 4, 4);
    assert_eq!(
        corner[3], 0,
        "corner outside brush footprint must be transparent; got {corner:?}"
    );
}

#[test]
fn ink_pen_preview_shows_color() {
    let out = render_preview("Ink Pen", 0.1, [0.0, 0.6, 0.0, 1.0]);
    let half = PREVIEW_SIDE / 2;
    let centre = px(&out.rgba, half, half);
    assert!(
        centre[1] > 100 && centre[0] < 30 && centre[3] > 200,
        "Ink Pen centre should be green-ish; got {centre:?}",
    );
}

#[test]
fn airbrush_preview_shows_color_with_alpha_falloff() {
    let out = render_preview("Airbrush", 0.1, [0.0, 0.0, 1.0, 1.0]);
    let half = PREVIEW_SIDE / 2;
    let centre = px(&out.rgba, half, half);
    assert!(
        centre[2] > 100 && centre[3] > 50,
        "Airbrush centre should have some blue with non-zero alpha; got {centre:?}",
    );
}

#[test]
fn rough_ink_preview_shows_color() {
    let out = render_preview("Rough Ink", 0.2, [0.7, 0.4, 0.0, 1.0]);
    let half = PREVIEW_SIDE / 2;
    let centre = px(&out.rgba, half, half);
    // Centre should be the brush color (orange-ish). Perlin noise can
    // dip flow at the centre — only assert color, not full opacity.
    assert!(
        centre[0] > centre[2] && centre[3] > 0,
        "Rough Ink centre should be visible and lean orange; got {centre:?}",
    );
    // (Boundary-variance check lives in `preview_watercolor.rs` against
    // Rough Watercolor — that brush has a much larger amplitude so the
    // shape modulation reads cleanly at our sampling resolution.)
}
