//! GPU integration tests for the Clone brush.
//!
//! The Clone brush reuses the `paint` terminal but sources each
//! fragment's colour from the frozen pre-stroke snapshot at a
//! stroke-constant offset (`clone_source` node). These tests paint a
//! known source patch, freeze it, set the clone anchors, stroke at a
//! destination offset, and assert the destination reads back as the
//! source colour shifted by the offset.
//!
//! The frame conversion (`uv = (src - source_offset) / source_size`) is
//! the recurring hazard, so the copy is exercised both at the origin and
//! under a **non-zero canvas origin + offset layer**. Erase mode is
//! covered too — clone inherits the terminal's paint-vs-erase commit.
//!
//! Engine-level tests below the runner-level ones cover cross-layer
//! source pinning, sample-merged, the deleted-source fallback, mask
//! (R8) sources, and pin revival through undo.

use std::sync::{Arc, OnceLock};

use darkly::brush::compile_graph;
use darkly::brush::eval::{BrushGraphRunner, CloneState};
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, StrokeResources};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::stroke_buffer::StrokeBuffer;
use darkly::coord::CanvasRect;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::paint_target::GpuPaintTarget;
use darkly::gpu::test_utils::{create_test_texture, readback_texture, test_device};
use darkly::layer::LayerId;

const W: u32 = 128;

fn shared_device() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    static HANDLES: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            let (d, q) = test_device();
            (Arc::new(d), Arc::new(q))
        })
        .clone()
}

/// Layer content in layer-local pixels: left half opaque red, right half
/// opaque black. The clone samples the red half and deposits it onto the
/// black half; erase removes the opaque black.
fn split_red_black() -> Vec<u8> {
    let mut out = vec![0u8; (W * W * 4) as usize];
    for y in 0..W {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            if x < W / 2 {
                out[i] = 255; // red
            }
            out[i + 3] = 255; // opaque
        }
    }
    out
}

struct CloneParams {
    /// Plane offset of the canvas window and the layer (kept equal so the
    /// window == layer; a non-zero value crops the plane).
    origin: [i32; 2],
    /// Clone source anchor in plane pixels.
    source_anchor: [f32; 2],
    /// First-dab destination anchor in plane pixels (also the dab centre).
    dest: [f32; 2],
    erase: bool,
}

/// Drive one clone dab through the runner directly (bypassing the stroke
/// engine so the anchors are set explicitly) and return the layer readback
/// in layer-local pixels.
fn render_clone(p: &CloneParams) -> Vec<u8> {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Clone")
        .expect("Clone builtin registered");

    let mut graph = brush.metadata.graph.clone();
    let _term_id = darkly::brush::find_terminal(&graph).expect("clone has a terminal");
    // Force a size that fits inside the half-canvas patch so the whole
    // dab footprint samples red.
    graph
        .set_port_default(
            &darkly::brush::nodes::brush_settings::node_id(&graph).unwrap(),
            "size",
            0.15,
        )
        .unwrap();

    let (device, queue) = shared_device();
    let (layer_texture, layer_view) =
        create_test_texture(&device, &queue, W, W, &split_red_black());
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut stroke_buffer = StrokeBuffer::new(
        &device,
        W,
        W,
        &pipelines,
        darkly::brush::node::COLOR_SCRATCH_FORMAT,
    );

    let layer_rect = CanvasRect::from_xywh(p.origin[0], p.origin[1], W, W);
    let pre_stroke = GpuPaintTarget::from_canvas_texture(
        &layer_texture,
        &layer_view,
        wgpu::TextureFormat::Rgba8Unorm,
        layer_rect,
    );
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("clone-pre-stroke"),
    });
    stroke_buffer.save_pre_stroke(&device, &mut enc, &pipelines, &pre_stroke);
    queue.submit([enc.finish()]);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("clone compiles");
    assert!(
        runner.samples_source(),
        "Clone brush must report samples_source"
    );
    // The source frame matches `layer_rect` — under a nonzero origin this
    // guards the uniform-carried frame the shader samples through (it no
    // longer reads `IntrinsicUniforms.layer_*`).
    runner.set_clone_state(Some(CloneState {
        source_anchor: p.source_anchor,
        dest_anchor: p.dest,
        source_offset: [p.origin[0] as f32, p.origin[1] as f32],
        source_size: [W as f32, W as f32],
    }));

    let blend_mode = u32::from(p.erase);
    macro_rules! make_ctx {
        ($label:expr) => {{
            let (scratch, pre_stroke_tex, pre_stroke_bg, source_override) =
                stroke_buffer.parts_for_brush_ctx();
            BrushGpuContext {
                encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some($label),
                }),
                device: &device,
                queue: &queue,
                pipelines: &pipelines,
                selection_bind_group: pipelines.default_selection_bind_group(),
                canvas_width: W,
                canvas_height: W,
                canvas_origin: p.origin,
                blend_mode,
                view_rotation: 0.0,
                perf: BrushPerfCounters::default(),
                stroke: Some(StrokeResources {
                    scratch,
                    paint_target: GpuPaintTarget::from_canvas_texture(
                        &layer_texture,
                        &layer_view,
                        wgpu::TextureFormat::Rgba8Unorm,
                        layer_rect,
                    ),
                    pre_stroke_texture: pre_stroke_tex,
                    pre_stroke_bind_group: pre_stroke_bg,
                    source_override,
                }),
                preview: None,
                dab_batch: DabBatch::default(),
            }
        }};
    }

    {
        let mut ctx = make_ctx!("clone-begin");
        runner.begin_stroke(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }
    {
        let mut ctx = make_ctx!("clone-dab");
        let info = PaintInformation {
            pos: p.dest,
            pressure: 1.0,
            ..Default::default()
        };
        runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0xC10E, 0);
        runner.execute_cpu();
        runner.execute_gpu(&mut ctx);
        runner.flush_dabs(&mut ctx);
        runner.commit(&mut ctx);
        queue.submit([ctx.encoder.finish()]);
    }

    readback_texture(
        &device,
        &queue,
        &layer_texture,
        wgpu::TextureFormat::Rgba8Unorm,
        W,
        W,
    )
}

/// Read a pixel at layer-local `(x, y)`.
fn local_px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn clone_copies_source_onto_destination() {
    // Source at (32, 64) in the red half; destination at (96, 64) in the
    // black half. Aligned mode, offset = source − dest = (−64, 0).
    let rgba = render_clone(&CloneParams {
        origin: [0, 0],
        source_anchor: [32.0, 64.0],
        dest: [96.0, 64.0],
        erase: false,
    });
    // Before clone the destination was black; after, it must read red.
    let dst = local_px(&rgba, 96, 64);
    assert!(
        dst[0] > 150 && dst[1] < 60 && dst[2] < 60,
        "cloned destination should be red (copied from the source patch), got {dst:?}"
    );
}

#[test]
fn clone_copies_under_nonzero_origin_and_offset_layer() {
    // Same geometry lifted into a cropped plane: the canvas window and the
    // layer both sit at plane (10, 20). This exercises the
    // `uv = (src - layer_offset) / layer_size` conversion — the recurring
    // wrong-frame hazard. Source/dest are given in plane pixels; the
    // readback is layer-local, so subtract the offset when probing.
    let origin = [10, 20];
    let rgba = render_clone(&CloneParams {
        origin,
        source_anchor: [10.0 + 32.0, 20.0 + 64.0],
        dest: [10.0 + 96.0, 20.0 + 64.0],
        erase: false,
    });
    let dst = local_px(&rgba, 96, 64);
    assert!(
        dst[0] > 150 && dst[1] < 60 && dst[2] < 60,
        "offset-layer clone destination should be red, got {dst:?}"
    );
}

// ============================================================================
// Engine-level: cross-layer pinning, sample merged, fallback, undo revival
// ============================================================================

/// Create a headless engine with the given canvas dimensions.
fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Node id of the first node with `type_id` in the active brush graph.
fn find_node_id(engine: &DarklyEngine, type_id: &str) -> String {
    engine
        .active_brush_graph()
        .nodes()
        .values()
        .find(|n| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in active graph"))
        .id
        .0
        .clone()
}

/// Install the builtin Clone brush as the active graph, with the tip
/// forced small enough that the whole dab footprint samples one flat
/// colour region (same sizing as the runner-level tests).
fn install_clone_brush(engine: &mut DarklyEngine) {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Clone")
        .expect("Clone builtin registered");
    let json = serde_json::to_string(&brush.metadata.graph).expect("serialize clone graph");
    engine.set_brush_graph(&json).expect("clone graph compiles");
    let term_id = find_node_id(engine, "paint");
    engine
        .brush_graph_set_input(
            &term_id,
            "size",
            darkly::brush::input_value::InputValue::Scalar(0.15),
        )
        .expect("paint size port");
}

/// One-dab stroke on `layer_id` at `(x, y)`. The stroke colour is
/// irrelevant for clone brushes (the source snapshot drives the colour)
/// but meaningful for the default brush used to seed layer content.
fn paint_dab(engine: &mut DarklyEngine, layer_id: LayerId, x: f32, y: f32, rgb: [f32; 3]) {
    engine.begin_stroke(layer_id);
    engine.stroke_to(StrokeOp::BrushStroke {
        x,
        y,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: rgb[0],
        cg: rgb[1],
        cb: rgb[2],
        ca: 1.0,
    });
    engine.end_stroke();
    engine.render(0.0);
}

/// RGBA at layer-local `(x, y)` from an engine layer readback.
fn engine_px(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// Pinning layer A and painting on layer B deposits A's pixels on B.
#[test]
fn cross_layer_clone_copies_pinned_layer() {
    let mut e = test_engine(W, W);
    let a = e.add_raster_layer(None);
    e.fill_background_color(a, [255, 0, 0, 255]);
    let b = e.add_raster_layer(None);
    e.render(0.0);

    install_clone_brush(&mut e);
    e.set_clone_source(32.0, 64.0, Some(a));
    paint_dab(&mut e, b, 96.0, 64.0, [1.0, 1.0, 1.0]);

    let px = engine_px(&e.test_readback_layer(b), W, 96, 64);
    assert!(
        px[0] > 150 && px[1] < 60 && px[2] < 60 && px[3] > 150,
        "painting on empty B with A pinned should deposit A's red, got {px:?}"
    );
}

/// With "Sample Merged" on, the clone reads the root composite — the top
/// layer's colour wins over the pinned (bottom) layer.
#[test]
fn sample_merged_clones_composite() {
    let mut e = test_engine(W, W);
    let bottom = e.add_raster_layer(None);
    e.fill_background_color(bottom, [255, 0, 0, 255]);
    let top = e.add_raster_layer(None);
    e.fill_background_color(top, [0, 0, 255, 255]);
    let dest = e.add_raster_layer(None);
    e.render(0.0);

    install_clone_brush(&mut e);
    let clone_id = find_node_id(&e, "clone_source");
    e.brush_graph_set_input(
        &clone_id,
        "merged",
        darkly::brush::input_value::InputValue::Scalar(1.0),
    )
    .expect("merged port");
    // Pin the red bottom layer to prove merged overrides the pin.
    e.set_clone_source(32.0, 64.0, Some(bottom));
    paint_dab(&mut e, dest, 96.0, 64.0, [1.0, 1.0, 1.0]);

    let px = engine_px(&e.test_readback_layer(dest), W, 96, 64);
    assert!(
        px[2] > 150 && px[0] < 60 && px[3] > 150,
        "merged clone should read the composite (blue top layer), got {px:?}"
    );
}

/// Deleting the pinned layer falls back to cloning the painted layer
/// itself (Krita's saved-node fallback) instead of failing or aliasing.
#[test]
fn deleted_source_falls_back_to_painted_layer() {
    let mut e = test_engine(W, W);
    let a = e.add_raster_layer(None);
    e.fill_background_color(a, [255, 0, 0, 255]);
    let b = e.add_raster_layer(None);
    // Seed B's own content at the source point with the default brush.
    paint_dab(&mut e, b, 32.0, 64.0, [0.0, 1.0, 0.0]);

    install_clone_brush(&mut e);
    e.set_clone_source(32.0, 64.0, Some(a));
    e.remove_layer(a).expect("remove pinned layer");
    paint_dab(&mut e, b, 96.0, 64.0, [1.0, 1.0, 1.0]);

    let px = engine_px(&e.test_readback_layer(b), W, 96, 64);
    assert!(
        px[1] > 150 && px[0] < 60,
        "with the pin dead, clone should sample B's own green, got {px:?}"
    );
}

/// A pinned mask (R8) broadcasts to grey through the shared snapshot
/// path — the default-white mask deposits white onto the raster layer.
#[test]
fn mask_source_broadcasts_r8() {
    let mut e = test_engine(W, W);
    let layer = e.add_raster_layer(None);
    e.add_mask(layer);
    let mask = e.test_mask_id(layer).expect("mask present");
    e.render(0.0);

    install_clone_brush(&mut e);
    e.set_clone_source(32.0, 64.0, Some(mask));
    paint_dab(&mut e, layer, 96.0, 64.0, [1.0, 0.0, 0.0]);

    let px = engine_px(&e.test_readback_layer(layer), W, 96, 64);
    assert!(
        px[0] > 200 && px[1] > 200 && px[2] > 200 && px[3] > 150,
        "mask source should broadcast the default-white mask as opaque white, got {px:?}"
    );
}

/// `LayerId` is generational: undoing the pinned layer's deletion
/// reinserts the same id, so the session pin revives with it.
#[test]
fn undo_restores_pinned_source() {
    let mut e = test_engine(W, W);
    let a = e.add_raster_layer(None);
    e.fill_background_color(a, [255, 0, 0, 255]);
    let b = e.add_raster_layer(None);
    e.render(0.0);

    install_clone_brush(&mut e);
    e.set_clone_source(32.0, 64.0, Some(a));
    e.remove_layer(a).expect("remove pinned layer");
    e.undo();
    e.render(0.0);
    paint_dab(&mut e, b, 96.0, 64.0, [1.0, 1.0, 1.0]);

    let px = engine_px(&e.test_readback_layer(b), W, 96, 64);
    assert!(
        px[0] > 150 && px[1] < 60 && px[2] < 60,
        "undoing A's deletion should revive the pin (deposit red), got {px:?}"
    );
}

#[test]
fn clone_erase_removes_at_destination() {
    // Erase mode: the clone coverage drives a destination-out commit, so
    // the opaque-black destination is removed (alpha drops toward 0).
    let rgba = render_clone(&CloneParams {
        origin: [0, 0],
        source_anchor: [32.0, 64.0],
        dest: [96.0, 64.0],
        erase: true,
    });
    let dst = local_px(&rgba, 96, 64);
    assert!(
        dst[3] < 40,
        "erase-mode clone should remove the destination pixel (alpha≈0), got {dst:?}"
    );
}
