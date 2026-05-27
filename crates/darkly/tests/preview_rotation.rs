//! End-to-end verification that `pen.tilt_direction` flows through
//! the runner's sensor-seeding into a compiled terminal's
//! `render_preview` and onward to `BrushPreviewInfo.rotation_rad`.
//!
//! This is the rotation-plumbing test the phase-5 plan calls out as
//! the prerequisite for the four per-terminal preview tests — if the
//! rotation port doesn't flow live values, the cursor mask can never
//! rotate with the pen even though the overlay primitive supports it.

use std::sync::Arc;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::wire::BrushWireType;
use darkly::brush::BrushNodeRegistry;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{Graph, PortRef};

const PREVIEW_SIDE: u32 = 128;

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

#[test]
fn pen_tilt_direction_drives_preview_rotation() {
    // Build a minimal paint_compiled graph that wires
    // `pen.tilt_direction → terminal.rotation`. The built-in brushes
    // don't wire rotation today, so the test constructs its own
    // graph rather than mutating a builtin's defaults.
    let registry = BrushNodeRegistry::new();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(0)],
    );
    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(0)],
    );
    let term = graph.add_node(
        "paint_compiled",
        registry.get("paint_compiled").unwrap().ports.clone(),
        vec![],
    );

    let wires = [
        (pen, "position", term, "position"),
        (pen, "tilt_direction", term, "rotation"),
        (paint_color, "color", stamp, "color"),
        (circle, "texture", stamp, "tip"),
        (stamp, "dab", term, "rgba"),
    ];
    for (from_node, from_port, to_node, to_port) in wires {
        graph
            .connect(
                PortRef {
                    node: from_node,
                    port: from_port.into(),
                },
                PortRef {
                    node: to_node,
                    port: to_port.into(),
                },
            )
            .unwrap();
    }
    graph.set_port_default(term, "size", 0.1).unwrap();

    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(&device, &queue);
    let (_target_tex, target_view) = preview_target(&device);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");

    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("preview-rotation"),
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

    // Non-zero pen tilt direction. The runner's `seed_sensors` writes
    // it straight into `pen_input.tilt_direction` and the wire
    // remap (no `natural_range` on tilt_direction) passes the value
    // through unchanged to the terminal's `rotation` input.
    let expected = 1.5_f32;
    let mut info = PaintInformation {
        pos: [64.0, 64.0],
        pressure: 1.0,
        ..Default::default()
    };
    info.tilt_direction = expected;
    runner.seed_sensors(&info, [1.0, 0.0, 0.0, 1.0], 0xC0FFEE, 0);
    runner.execute_cpu();
    runner.render_preview_pipeline(&mut ctx);

    let rotation = ctx
        .brush_preview_info
        .expect("paint_compiled publishes brush_preview_info during preview")
        .rotation_rad;

    // Without the plumbing this would be 0; with it the seeded
    // tilt_direction lands here. Allow a small epsilon for
    // wire-remap float noise.
    assert!(
        (rotation - expected).abs() < 1e-3,
        "expected rotation ≈ {expected}, got {rotation}",
    );
    // Drain the encoder so the device doesn't complain on drop.
    queue.submit([ctx.encoder.finish()]);
}
