//! Hover-cursor preview render through `watercolor_compiled`.
//! Verifies the override `compile_preview_body` (which drops the
//! `@group(3)` pickup-atlas sample the stroke body uses) produces a
//! color-modulated mask, and Rough Watercolor's perlin shape
//! produces visible boundary variance.

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
        .find(|(_, n)| n.type_id == "watercolor_compiled")
        .map(|(id, _)| *id)
        .expect("brush terminates in watercolor_compiled");
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
        label: Some("preview-watercolor"),
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
        .expect("watercolor_compiled publishes brush_preview_info");
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
fn smooth_watercolor_preview_shows_color() {
    let out = render_preview("Smooth Watercolor", 0.1, [0.2, 0.3, 0.9, 1.0]);
    let half = PREVIEW_SIDE / 2;
    let centre = px(&out.rgba, half, half);
    // Smooth Watercolor: pressure=1 → flow ~ 1 → centre is brush color
    // (premultiplied). Don't assert near-opaque alpha — the preview
    // body folds mask × flow × color.a into the alpha and edge
    // softness brings the centre down slightly from full.
    assert!(
        centre[2] > centre[0] && centre[2] > 30 && centre[3] > 30,
        "Smooth Watercolor centre should lean blue with non-zero alpha; got {centre:?}",
    );
    assert!(out.info.half_extent_canvas_px[0] > 0.0);
}

#[test]
fn rough_watercolor_preview_shows_color_and_silhouette_variance() {
    let out = render_preview("Rough Watercolor", 0.2, [0.9, 0.3, 0.2, 1.0]);
    let half = PREVIEW_SIDE / 2;
    let centre = px(&out.rgba, half, half);
    // Centre should lean red (the seeded brush color).
    assert!(
        centre[0] > centre[2] && centre[3] > 0,
        "Rough Watercolor centre should be visible and lean red; got {centre:?}",
    );

    // Rough Watercolor's perlin shape (amplitude 0.4, frequency 12)
    // should produce visible boundary variance at intermediate radii
    // — sample ~75% of `radius` (= `bbox_half / brush_extent_factor *
    // 0.75`). At amplitude 0.4 the shape boundary swings [0.6, 1.4]
    // × radius, so 0.75 × radius is inside at some thetas, outside
    // at others.
    //
    // `brush_extent_factor` for amplitude_max=0.5 is 1.5, so the
    // factor here is 1.4 (saved on the brush via `port_max_value`).
    // Sampling at 0.5 × bbox_half puts us at ~0.71 × radius — safely
    // inside the average shape, with the perlin dips visible.
    // Sweep multiple radii. The exact boundary radius isn't known a
    // priori (perlin sampling varies per dab/seed); at *some* radius
    // we straddle the shape's wavy boundary and see opaque-and-
    // transparent neighbours along theta. Pick the widest variance
    // across the sweep — if the cursor were a perfect disc, every
    // sample at one radius would be either all opaque or all
    // transparent.
    let bbox_half = out.info.half_extent_canvas_px[0];
    let mut best_variance = 0_u8;
    for r_frac_x10 in 50..=90 {
        let r_sample = bbox_half * (r_frac_x10 as f32 / 100.0);
        let mut min_a = 255_u8;
        let mut max_a = 0_u8;
        let theta_steps = 32;
        for i in 0..theta_steps {
            let theta = (i as f32 / theta_steps as f32) * std::f32::consts::TAU;
            let sx = (half as f32 + r_sample * theta.cos()) as i32;
            let sy = (half as f32 + r_sample * theta.sin()) as i32;
            if sx >= 0 && sx < PREVIEW_SIDE as i32 && sy >= 0 && sy < PREVIEW_SIDE as i32 {
                let a = px(&out.rgba, sx as u32, sy as u32)[3];
                min_a = min_a.min(a);
                max_a = max_a.max(a);
            }
        }
        let variance = max_a - min_a;
        if variance > best_variance {
            best_variance = variance;
        }
    }
    assert!(
        best_variance > 40,
        "Rough Watercolor preview should show variance along the perlin \
         boundary (a perfect disc would have variance 0); got max variance \
         {best_variance} across r in [0.5, 0.9] × bbox_half",
    );
}
