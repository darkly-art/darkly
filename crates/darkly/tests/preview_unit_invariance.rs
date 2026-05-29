//! Regression test for the brush preview unit-mismatch bug.
//!
//! The bug: when the preview-mask texture is smaller than the brush's
//! canvas-px bbox (the production `MAX_PREVIEW_MASK_SIDE = 512` clamp,
//! or any test-allocated mask sized below the brush's natural bbox),
//! the fragment shader's discard test compared a target-pixel `local`
//! against a canvas-pixel `bbox_radius` from the dab record. The
//! discard never fired and the dab filled the texture to its square
//! edge — visible as "square-clipped" cursor previews at large brush
//! sizes.
//!
//! The fix: the intrinsic dab header is packed in the target's pixel
//! space (canvas px for stroke, mask texels for preview). The bug
//! becomes structurally inexpressible. This test exercises the
//! preview path with a deliberately undersized mask to confirm the
//! discard now fires correctly.
//!
//! **Pre-fix**: assertion 1 (corners transparent) FAILS — corners
//! come back nearly opaque because the discard never fires.

use std::sync::Arc;

use darkly::brush::compile_graph;
use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::gpu_context::{BrushGpuContext, BrushPerfCounters, DabBatch, PreviewState};
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::pipeline::BrushPipelines;
use darkly::gpu::test_utils::{readback_texture, test_device};

/// Deliberately undersized vs the brush's natural bbox — simulates the
/// production `MAX_PREVIEW_MASK_SIDE` clamp without needing a real
/// `ToolOverlay`. A 128² mask + a brush with canvas-px bbox ~512 puts
/// the dab well over the texture's inscribed disc (`texture_half = 64`).
const PREVIEW_SIDE: u32 = 128;

fn preview_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("preview-unit-invariance-mask"),
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

struct Out {
    rgba: Vec<u8>,
    half_extent_canvas_px: [f32; 2],
}

/// Render `Round` at a brush size whose canvas-px bbox exceeds the
/// 128² test mask's inscribed half-side (64). The exact size value
/// is pinned so future default-tuning of `Round` doesn't drift the
/// assertions.
fn render_big_round() -> Out {
    let brush = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Round")
        .expect("Round brush is registered");
    let mut graph = brush.metadata.graph.clone();
    let term_id = darkly::brush::find_terminal(&graph).expect("Round has a terminal");
    // size 2.0 → effective_radius = 2.0 * 512 * 0.5 = 512 canvas px
    // (extent factor ≥ 1, so bbox_canvas_px is at least 512, far above
    // texture_half = 64).
    graph.set_port_default(term_id, "size", 2.0).unwrap();

    let (device, queue) = test_device();
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let pipelines = BrushPipelines::new(&device, &queue);
    let (target_tex, target_view) = preview_target(&device);

    let mut runner: BrushGraphRunner = compile_graph(&graph).expect("brush compiles");

    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("preview-unit-invariance"),
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
        // Drive the test-fallback path on `ensure_preview_mask` — the
        // production clamp path needs a real `ToolOverlay`, which is
        // heavier to construct than this test needs. The undersized
        // mask reproduces the exact same target-vs-canvas unit
        // mismatch the production clamp triggers.
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
    runner.seed_sensors(&info, [1.0, 1.0, 1.0, 1.0], 0xABCDEF, 0);
    runner.execute_cpu();
    runner.render_preview_pipeline(&mut ctx);
    let published = ctx
        .preview
        .as_ref()
        .and_then(|p| p.info)
        .expect("render_compiled_preview publishes BrushPreviewInfo");
    queue.submit([ctx.encoder.finish()]);

    let rgba = readback_texture(
        &device,
        &queue,
        &target_tex,
        wgpu::TextureFormat::Rgba8Unorm,
        PREVIEW_SIDE,
        PREVIEW_SIDE,
    );
    Out {
        rgba,
        half_extent_canvas_px: published.half_extent_canvas_px,
    }
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * PREVIEW_SIDE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn large_brush_does_not_fill_preview_mask_with_square() {
    let out = render_big_round();

    // The overlay consumer-facing bbox stays in canvas px. With size
    // 2.0 → radius 512 and extent factor ≥ 1, the published bbox
    // half-extent is well above the 128 mask's texture_half (64).
    // This is what tells the overlay to display a quad bigger than
    // the mask, which gets bilinearly upsampled.
    assert!(
        out.half_extent_canvas_px[0] > 256.0,
        "expected canvas-px bbox >> texture half; got {:?}",
        out.half_extent_canvas_px
    );

    // ── Headline regression: the four corners must be transparent.
    //
    // The corners sit at distance ~85 from the centre (64, 64) — well
    // outside the dab's `bbox_target_px = 64`, so the fragment discard
    // fires and writes nothing.
    //
    // Pre-fix: `bbox_target_px` was packed as the canvas-px value
    // (≈512), the discard never fired within the 128² texture, and the
    // brush body painted opaque white at every corner. Alpha → ~255.
    for (cx, cy) in [(0, 0), (127, 0), (0, 127), (127, 127)] {
        let p = px(&out.rgba, cx, cy);
        assert_eq!(
            p[3], 0,
            "corner ({cx}, {cy}) must be transparent — discard fires past `bbox_target_px`; got {p:?}",
        );
    }

    // ── Sanity: the dab actually rendered something at the centre.
    // Without this, a regression that turned every fragment into a
    // no-op would satisfy the corner check above.
    let centre = px(&out.rgba, PREVIEW_SIDE / 2, PREVIEW_SIDE / 2);
    assert!(
        centre[3] > 0,
        "centre must be inside the dab and have non-zero alpha; got {centre:?}",
    );

    // ── Radial symmetry. Round has no angular dependence; four
    // symmetric points well inside the bbox should agree closely.
    // Pick offset 32 from centre (half the bbox half-extent) — deep
    // in the falloff, so any rotational asymmetry would read clearly
    // but we're not sitting on the discard boundary.
    let half = PREVIEW_SIDE / 2;
    let r: u32 = 32;
    let north = px(&out.rgba, half, half - r);
    let south = px(&out.rgba, half, half + r);
    let east = px(&out.rgba, half + r, half);
    let west = px(&out.rgba, half - r, half);
    let max_dev = [north[3], south[3], east[3], west[3]]
        .iter()
        .map(|&a| (a as i32 - north[3] as i32).abs())
        .max()
        .unwrap();
    assert!(
        max_dev <= 3,
        "axial samples at radius {r} should be ~equal (Round is rotationally invariant); \
         N={north:?} S={south:?} E={east:?} W={west:?}",
    );
}
