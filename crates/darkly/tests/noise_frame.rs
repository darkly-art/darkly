//! End-to-end verification that a **Dab-space** `noise` node locks its grain
//! to the stamp: wiring `pen.drawing_angle → noise.rotation` must rotate the
//! rendered noise pattern with the dab, instead of leaving it pinned to the
//! canvas (the reported "grain swims under the rotating stamp" bug).
//!
//! The graph has no shape node, so the dab silhouette (a full disc) is
//! identical between the two renders — only the interior grain differs. The
//! probe therefore compares **RGB** (the noise color), not alpha coverage.
//! At the dab centre the oriented frame is invariant, so probes are taken
//! off-centre where rotation actually moves the sample.

use std::sync::Arc;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{
    BrushGpuContext, BrushPerfCounters, CursorPreviewState, DabBatch,
};
use darkly::brush::input_value::InputValue;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::registry;
use darkly::brush::wire::BrushWireType;
use darkly::gpu::test_utils::{readback_texture, test_device};
use darkly::nodegraph::{Graph, PortRef};

const PREVIEW_SIDE: u32 = 128;

fn preview_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("noise-frame-target"),
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

/// Dab-space noise → paint, with `pen.drawing_angle → noise.rotation`.
/// `scale_with_brush = false` and a small `scale` pack several fBm cells
/// across the stamp so rotation produces a strong, easily-probed change.
fn build_dab_noise_graph() -> Graph<BrushWireType> {
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node("pen_input", reg.get("pen_input").unwrap().ports.clone());
    graph.add_node(
        "brush_settings",
        reg.get("brush_settings").unwrap().ports.clone(),
    );
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());
    // scale small ⇒ many cells across the stamp; space = Dab, pixel-locked.
    for (name, v) in [
        ("scale", InputValue::Scalar(6.0)),
        ("seed", InputValue::Int(7)),
        ("octaves", InputValue::Scalar(4.0)),
        ("warp", InputValue::Scalar(0.6)),
        ("roughness", InputValue::Scalar(0.5)),
        ("space", InputValue::Int(1)),
        ("scale_with_brush", InputValue::Bool(false)),
    ] {
        graph.set_port_value(noise, name, v).unwrap();
    }
    let term = graph.add_node("paint", reg.get("paint").unwrap().ports.clone());
    let wires = [
        (pen, "position", term, "position"),
        (pen, "drawing_angle", noise, "rotation"),
        (noise, "color", term, "rgba"),
    ];
    for (fnode, fport, tnode, tport) in wires {
        graph
            .connect(
                PortRef {
                    node: fnode,
                    port: fport.into(),
                },
                PortRef {
                    node: tnode,
                    port: tport.into(),
                },
            )
            .unwrap();
    }
    // Big dab so the grain fills most of the preview texture.
    graph
        .set_port_default(
            darkly::brush::nodes::brush_settings::node_id(&graph).unwrap(),
            "size",
            0.45,
        )
        .unwrap();
    graph
}

fn render_at_angle(angle_rad: f32) -> Vec<u8> {
    let graph = build_dab_noise_graph();
    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let (target_tex, target_view) = preview_target(&device);
    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");

    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("noise-frame"),
    });
    let mut ctx = BrushGpuContext {
        encoder,
        device: &device,
        queue: &queue,
        pipelines: &pipelines,
        selection_bind_group: pipelines.default_selection_bind_group(),
        canvas_width: PREVIEW_SIDE,
        canvas_height: PREVIEW_SIDE,
        canvas_origin: [0, 0],
        blend_mode: 0,
        view_rotation: 0.0,
        perf: BrushPerfCounters::default(),
        stroke: None,
        preview: Some(CursorPreviewState {
            mask_view: Some(&target_view),
            mask_size: (PREVIEW_SIDE, PREVIEW_SIDE),
            mask_overlay: None,
            info: None,
        }),
        dab_batch: DabBatch::default(),
    };

    let mut info = PaintInformation {
        pos: [PREVIEW_SIDE as f32 * 0.5, PREVIEW_SIDE as f32 * 0.5],
        pressure: 1.0,
        ..Default::default()
    };
    info.drawing_angle = angle_rad;
    runner.seed_sensors(&info, [1.0, 0.0, 0.0, 1.0], 0xC0FFEE, 0);
    runner.execute_cpu();
    runner.render_cursor_preview_pipeline(&mut ctx);
    queue.submit([ctx.encoder.finish()]);

    readback_texture(
        &device,
        &queue,
        &target_tex,
        wgpu::TextureFormat::Rgba8Unorm,
        PREVIEW_SIDE,
        PREVIEW_SIDE,
    )
}

fn rgb(rgba: &[u8], x: u32, y: u32) -> [i32; 3] {
    let i = ((y * PREVIEW_SIDE + x) * 4) as usize;
    [rgba[i] as i32, rgba[i + 1] as i32, rgba[i + 2] as i32]
}

#[test]
fn dab_space_noise_rotates_with_drawing_angle() {
    let baseline = render_at_angle(0.0);
    let rotated = render_at_angle(std::f32::consts::FRAC_PI_2); // 90°

    // Off-centre probes at ~half the dab extent. The disc coverage is
    // identical between renders, but Dab-space grain rotates about the
    // centre, so these interior samples land on different parts of the
    // field and must differ in colour.
    let half = PREVIEW_SIDE as i32 / 2;
    let r = (PREVIEW_SIDE as i32 / 5).max(8);
    let probes = [
        (half + r, half),
        (half - r, half),
        (half, half + r),
        (half, half - r),
        (half + r, half + r),
        (half - r, half - r),
    ];

    let mut total_diff: i64 = 0;
    for (x, y) in probes {
        let a = rgb(&baseline, x as u32, y as u32);
        let b = rgb(&rotated, x as u32, y as u32);
        for c in 0..3 {
            total_diff += (a[c] - b[c]).unsigned_abs() as i64;
        }
    }

    assert!(
        total_diff > 64,
        "Dab-space noise wired to pen.drawing_angle must rotate the grain \
         with the stamp; summed |ΔRGB| across off-centre probes was \
         {total_diff} (near zero ⇒ the frame is still canvas-locked and the \
         rotation is dead)",
    );
}
