//! Probe: dump alpha/red values across a horizontal slice of the
//! liquify preview at softness=0.

use std::sync::Arc;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::gpu::test_utils::{readback_texture, test_device};

const PREVIEW_SIDE: u32 = 256;

#[test]
fn probe_softness_zero() {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Liquify")
        .unwrap();
    let mut graph = brush.metadata.graph.clone();
    let term_id = graph
        .nodes
        .iter()
        .find(|(_, n)| n.type_id == "liquify_compiled")
        .map(|(id, _)| *id)
        .unwrap();
    graph.set_port_default(term_id, "size", 0.3).unwrap();
    graph.set_port_default(term_id, "softness", 0.0).unwrap();

    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(&device, &queue);
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe"),
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

    let mut runner: BrushGraphRunner = compile_graph(&graph).unwrap();
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
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
        preview_target_view: Some(&view),
        blend_mode: 0,
        preview_mask_view: Some(&view),
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
        pos: [128.0, 128.0],
        pressure: 1.0,
        ..Default::default()
    };
    runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0, 0);
    runner.execute_cpu();
    runner.render_preview_pipeline(&mut ctx);
    let bpi = ctx.brush_preview_info.unwrap();
    eprintln!("bbox_half = {:?}", bpi.half_extent_canvas_px);
    queue.submit([ctx.encoder.finish()]);
    let rgba = readback_texture(
        &device,
        &queue,
        &tex,
        wgpu::TextureFormat::Rgba8Unorm,
        PREVIEW_SIDE,
        PREVIEW_SIDE,
    );
    // Per-row opaque x range.
    for y in [51, 52, 60, 80, 128, 175, 200, 204, 205] {
        let mut row_first = None;
        let mut row_last = None;
        for x in 0..PREVIEW_SIDE {
            let i = ((y * PREVIEW_SIDE + x) * 4) as usize;
            if rgba[i + 3] > 0 {
                if row_first.is_none() {
                    row_first = Some(x);
                }
                row_last = Some(x);
            }
        }
        eprintln!("y={:>3}: opaque x range: {:?}..={:?}", y, row_first, row_last);
    }
}
