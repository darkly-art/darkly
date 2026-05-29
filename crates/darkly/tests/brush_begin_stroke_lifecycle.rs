//! Regression test for the framework-managed `begin_stroke` lifecycle.
//!
//! Before this refactor, each of the four scratch-touching terminals
//! (`paint`, `watercolor`, `smudge`, `liquify`) carried its own
//! `begin_stroke` impl with copy-pasted scratch prep — clear-to-transparent
//! for paint/watercolor, copy-from-pre-stroke for smudge/liquify. Commit
//! `24ccdcf` ("fix other watercolor bug") landed a literal copy-paste of
//! paint's clear pass into watercolor because the prologue had silently
//! diverged. Stage 2 of the registration unification moves the prologue
//! into a framework hook driven by `BrushNodeRegistration::lifecycle`, so
//! adding a new terminal can't re-introduce the divergence.
//!
//! These tests assert the *framework-observable* effect of `begin_stroke`
//! on the scratch — independent of whichever terminal's `begin_stroke`
//! impl ran (after Stage 2, all four are the trait default no-op).

use std::sync::Arc;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters};
use darkly::brush::pipeline::BrushPipelines;
use darkly::brush::scratch::Scratch;
use darkly::brush::wire::BrushWireType;
use darkly::gpu::test_utils::{readback_texture, test_device};
use darkly::nodegraph::Graph;

const W: u32 = 32;
const H: u32 = 32;

/// Magenta sentinel — distinct from the transparent-black clear and from
/// any incidental zero data, so a missed framework hook is unambiguous.
const SENTINEL_RGBA: [u8; 4] = [255, 0, 255, 255];

fn paint_solid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    view: &wgpu::TextureView,
    color: wgpu::Color,
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("paint-solid"),
    });
    let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("paint-solid-pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(color),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    queue.submit([encoder.finish()]);
}

fn make_pre_stroke(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pre-stroke-tex"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

enum Setup {
    /// Pre-fill the scratch with `color`; no pre-stroke texture.
    ScratchPrefilled(wgpu::Color),
    /// Allocate a pre-stroke texture and fill it with the sentinel color;
    /// scratch starts at its default (transparent).
    PreStrokeWithSentinel,
}

fn run_begin_stroke(graph: &Graph<BrushWireType>, setup: Setup) -> Vec<u8> {
    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(&device, &queue);
    let mut scratch = Scratch::new(
        &device,
        W,
        H,
        pipelines.canvas_copy_bind_group_layout(),
        pipelines.canvas_copy_sampler(),
    );

    // Build pre-stroke (if needed) and pre-fill scratch (if needed) on
    // the same device/queue the runner uses — wgpu rejects cross-device
    // resource use.
    let pre_stroke = match &setup {
        Setup::PreStrokeWithSentinel => {
            let (tex, view) = make_pre_stroke(&device);
            paint_solid(
                &device,
                &queue,
                &view,
                wgpu::Color {
                    r: SENTINEL_RGBA[0] as f64 / 255.0,
                    g: SENTINEL_RGBA[1] as f64 / 255.0,
                    b: SENTINEL_RGBA[2] as f64 / 255.0,
                    a: SENTINEL_RGBA[3] as f64 / 255.0,
                },
            );
            Some(tex)
        }
        Setup::ScratchPrefilled(color) => {
            paint_solid(&device, &queue, scratch.write_view(), *color);
            None
        }
    };

    let mut runner: BrushGraphRunner = compile_graph(graph).expect("brush compiles");
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("begin-stroke"),
    });
    let mut ctx = BrushGpuContext {
        encoder,
        device: &device,
        queue: &queue,
        pipelines: &pipelines,
        scratch: Some(&mut scratch),
        canvas_width: W,
        canvas_height: H,
        paint_target: None,
        selection_bind_group: pipelines.default_selection_bind_group(),
        preview_target_view: None,
        blend_mode: 0,
        preview_mask_view: None,
        preview_mask_size: (0, 0),
        preview_mask_overlay: None,
        brush_preview_info: None,
        pre_stroke_texture: pre_stroke.as_ref(),
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
    runner.begin_stroke(&mut ctx);
    queue.submit([ctx.encoder.finish()]);

    readback_texture(
        &device,
        &queue,
        scratch.write_texture(),
        wgpu::TextureFormat::Rgba8Unorm,
        W,
        H,
    )
}

fn builtin_graph(name: &str) -> Graph<BrushWireType> {
    darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == name)
        .unwrap_or_else(|| panic!("builtin brush `{name}` not registered"))
        .metadata
        .graph
}

fn assert_all(rgba: &[u8], expected: [u8; 4]) {
    let mut mismatches = 0usize;
    let mut first_bad = None;
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        if px != expected {
            if first_bad.is_none() {
                first_bad = Some((i, [px[0], px[1], px[2], px[3]]));
            }
            mismatches += 1;
        }
    }
    assert_eq!(
        mismatches, 0,
        "{} pixels differ from expected {:?}; first at idx {:?}",
        mismatches, expected, first_bad
    );
}

#[test]
fn paint_terminal_clears_scratch_to_transparent() {
    // Pre-fill with magenta so a missed clear is visible. The framework
    // lifecycle for `Lifecycle::ClearScratchToTransparent` must wipe it
    // back to (0, 0, 0, 0).
    let rgba = run_begin_stroke(
        &builtin_graph("Round"),
        Setup::ScratchPrefilled(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        }),
    );
    assert_all(&rgba, [0, 0, 0, 0]);
}

#[test]
fn watercolor_terminal_clears_scratch_to_transparent() {
    // The watercolor regression that originally motivated this refactor:
    // before commit 24ccdcf, watercolor's begin_stroke was missing the
    // clear pass paint had, leaving stale pigment in the scratch after
    // a rewind boundary. Now the clear is framework-driven, so removing
    // any single terminal's begin_stroke can never resurrect the bug.
    let rgba = run_begin_stroke(
        &builtin_graph("Smooth Watercolor"),
        Setup::ScratchPrefilled(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        }),
    );
    assert_all(&rgba, [0, 0, 0, 0]);
}

#[test]
fn smudge_terminal_seeds_scratch_from_pre_stroke() {
    let rgba = run_begin_stroke(&builtin_graph("Smudge"), Setup::PreStrokeWithSentinel);
    assert_all(&rgba, SENTINEL_RGBA);
}

#[test]
fn liquify_terminal_seeds_scratch_from_pre_stroke() {
    let rgba = run_begin_stroke(&builtin_graph("Liquify"), Setup::PreStrokeWithSentinel);
    assert_all(&rgba, SENTINEL_RGBA);
}
