//! Hover-cursor preview render through `liquify`. The
//! preview body shows the brush's softness-shaped disc in neutral
//! gray — same `falloff_fn` the stroke body emits as a node decl,
//! so scrubbing softness visibly reshapes the cursor.

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

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * PREVIEW_SIDE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn liquify_preview_shows_neutral_gray_disc() {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Liquify")
        .expect("Liquify brush registered");
    let mut graph = brush.metadata.graph.clone();
    let term_id = darkly::brush::find_terminal(&graph).expect("brush has a terminal");
    graph.set_port_default(term_id, "size", 0.2).unwrap();

    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(&device, &queue);
    let (target_tex, target_view) = preview_target(&device);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("preview-liquify"),
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
    runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0xC0FFEE, 0);
    runner.execute_cpu();
    runner.render_preview_pipeline(&mut ctx);
    let _published = ctx
        .brush_preview_info
        .expect("liquify publishes brush_preview_info");
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
    // Centre with softness=0.5 (default) produces a peaked falloff —
    // f(0) ≈ 1, so centre ≈ (153, 153, 153, 255).
    assert!(
        centre[3] > 100,
        "Liquify preview centre should have meaningful alpha; got {centre:?}",
    );
    assert!(
        (centre[0] as i32 - centre[1] as i32).abs() < 5
            && (centre[1] as i32 - centre[2] as i32).abs() < 5,
        "Liquify preview should be neutral gray; got {centre:?}",
    );

    // The disc has finite extent (bbox_radius). Sample a corner of
    // the mask well outside the brush — must be transparent.
    let corner = px(&rgba, 4, 4);
    assert_eq!(
        corner[3], 0,
        "Liquify preview corner should be transparent; got {corner:?}",
    );
}

#[test]
fn liquify_preview_softness_reshapes_falloff() {
    // Sample two extremes of softness and confirm the falloff curve
    // differs at the disc's mid-radius. softness=0 → hard / uniform;
    // softness=1 → spike at centre, near-zero past mid-radius. Mid-
    // radius alpha should differ noticeably between the two.
    fn mid_radius_alpha(softness: f32) -> u8 {
        let brush = darkly::brush::builtin_brushes::all()
            .into_iter()
            .find(|b| b.metadata.name == "Liquify")
            .unwrap();
        let mut graph = brush.metadata.graph.clone();
        let term_id = darkly::brush::find_terminal(&graph).unwrap();
        graph.set_port_default(term_id, "size", 0.2).unwrap();
        graph
            .set_port_default(term_id, "softness", softness)
            .unwrap();

        let (device, queue) = test_device();
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let pipelines = BrushPipelines::new(&device, &queue);
        let (target_tex, target_view) = preview_target(&device);
        let mut runner: BrushGraphRunner = compile_graph(&graph).unwrap();
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview-liquify-soft"),
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
        runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0xC0FFEE, 0);
        runner.execute_cpu();
        runner.render_preview_pipeline(&mut ctx);
        let half_extent = ctx
            .brush_preview_info
            .expect("liquify publishes preview info")
            .half_extent_canvas_px[0];
        queue.submit([ctx.encoder.finish()]);
        let rgba = readback_texture(
            &device,
            &queue,
            &target_tex,
            wgpu::TextureFormat::Rgba8Unorm,
            PREVIEW_SIDE,
            PREVIEW_SIDE,
        );
        let half = (PREVIEW_SIDE / 2) as f32;
        let r_mid = half_extent * 0.5;
        px(&rgba, (half + r_mid) as u32, half as u32)[3]
    }

    let hard_alpha = mid_radius_alpha(0.0);
    let soft_alpha = mid_radius_alpha(1.0);
    // Hard (uniform): mid-radius alpha ≈ centre alpha (~153).
    // Soft (spike):   mid-radius alpha ≈ 0 (the spike has decayed
    // away by the half-radius).
    assert!(
        (hard_alpha as i32 - soft_alpha as i32).abs() > 50,
        "softness scrub should reshape the cursor's mid-radius alpha; \
         got hard={hard_alpha} soft={soft_alpha}",
    );
}
