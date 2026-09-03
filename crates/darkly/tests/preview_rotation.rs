//! End-to-end verification that `pen.drawing_angle → circle.rotation_input`
//! actually rotates the **rendered mask** in the cursor preview pipeline.
//!
//! The previous version of this test asserted on
//! `BrushCursorPreviewInfo.rotation_rad`, a CPU-side overlay rotation that
//! only affected the cursor halo, not the painted dab. That port was removed
//! because it produced "rotation that only the cursor sees, not the paint",
//! which surprised artists. Rotation now lives in the WGSL pipeline (sum of
//! `circle.rotation` + `circle.rotation_input`, minus `view_rotation`), so
//! the cursor preview and the stroke deposit always agree.
//!
//! This test renders an asymmetric superformula shape twice (once
//! unrotated, once at 90° via `pen.drawing_angle`) and asserts that
//! mask pixels at on-axis sampling positions differ between the two
//! renders. If `drawing_angle → rotation_input` were silently dropped,
//! both renders would be identical.

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

/// Build a minimal paint graph with `pen.drawing_angle → circle.rotation_input`.
/// Shape is set to the superformula algorithm so the silhouette has a four-fold
/// symmetry whose orientation reads cleanly off rotation_input.
fn build_graph_with_rotation_wire() -> Graph<BrushWireType> {
    let registry = registry();
    let mut graph = Graph::<BrushWireType>::new();

    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
    );
    graph.add_node(
        "brush_settings",
        registry.get("brush_settings").unwrap().ports.clone(),
    );
    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
    );
    let shape = graph.add_node("circle", registry.get("circle").unwrap().ports.clone());
    // Algorithm = 2 (Superformula).
    graph
        .set_port_value(&shape, "algorithm", InputValue::Int(2))
        .unwrap();
    let stamp = graph.add_node("stamp", registry.get("stamp").unwrap().ports.clone());
    let term = graph.add_node("paint", registry.get("paint").unwrap().ports.clone());

    let wires = [
        (pen.clone(), "position", term.clone(), "position"),
        // Canonical stroke-follow wire: what artists wire when they want
        // the dab to face the stroke direction.
        (
            pen.clone(),
            "drawing_angle",
            shape.clone(),
            "rotation_input",
        ),
        (paint_color.clone(), "color", stamp.clone(), "color"),
        (shape.clone(), "mask", stamp.clone(), "tip"),
        (stamp.clone(), "dab", term.clone(), "rgba"),
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
    // Big size so the mask occupies most of the preview texture.
    graph
        .set_port_default(
            &darkly::brush::nodes::brush_settings::node_id(&graph).unwrap(),
            "size",
            0.4,
        )
        .unwrap();
    // Superformula with a 4-pointed silhouette so rotation moves the
    // shape's lobes from axes to diagonals (and vice versa).
    graph.set_port_default(&shape, "frequency", 4.0).unwrap();
    graph.set_port_default(&shape, "amplitude", 0.8).unwrap();
    graph.set_port_default(&shape, "n1", 1.0).unwrap();
    graph.set_port_default(&shape, "n2", 1.0).unwrap();
    graph.set_port_default(&shape, "n3", 1.0).unwrap();
    graph
}

/// Render the preview mask with `info.drawing_angle = angle_rad` and
/// return the RGBA8 bytes.
fn render_at_angle(angle_rad: f32) -> Vec<u8> {
    let graph = build_graph_with_rotation_wire();

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
        label: Some("preview-rotation"),
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

    // Seed `drawing_angle` directly: `seed_sensors` writes the raw
    // `info.drawing_angle` into the pen_input slot, which the wire then
    // feeds into `circle.rotation_input` via the compiled WGSL.
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

fn alpha(rgba: &[u8], x: u32, y: u32) -> u8 {
    let i = ((y * PREVIEW_SIDE + x) * 4) as usize;
    rgba[i + 3]
}

#[test]
fn drawing_angle_rotates_preview_mask() {
    let baseline = render_at_angle(0.0);
    let rotated = render_at_angle(std::f32::consts::FRAC_PI_4); // 45°

    // Average alpha over an off-axis arc at ~half the brush extent.
    // A 4-pointed superformula has its lobes aligned with ±x / ±y at
    // rotation=0; at 45° those lobes swing onto the diagonals. So a
    // sample taken at an axis-aligned offset will be inside the shape
    // in one render and outside in the other.
    //
    // Sample a small disc of pixels to absorb the smoothstep edge band;
    // single-pixel reads are too noisy on the AA boundary.
    let half = PREVIEW_SIDE as i32 / 2;
    let probe_r = (PREVIEW_SIDE as i32 / 5).max(8);
    let probe_samples = [
        (half + probe_r, half),
        (half - probe_r, half),
        (half, half + probe_r),
        (half, half - probe_r),
    ];

    let mut sum_baseline: u32 = 0;
    let mut sum_rotated: u32 = 0;
    for (x, y) in probe_samples {
        sum_baseline += alpha(&baseline, x as u32, y as u32) as u32;
        sum_rotated += alpha(&rotated, x as u32, y as u32) as u32;
    }

    let diff = (sum_baseline as i32 - sum_rotated as i32).unsigned_abs();
    assert!(
        diff > 64,
        "drawing_angle wired to circle.rotation_input should rotate the \
         rendered shape; baseline on-axis alpha={sum_baseline}, rotated \
         on-axis alpha={sum_rotated} (diff={diff}). If these are equal, \
         the rotation wire is dead.",
    );
}
