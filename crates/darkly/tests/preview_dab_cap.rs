//! A dab phase longer than `MAX_DABS_PER_PHASE` must split, not overflow.
//!
//! Dab-batching terminals keep their queue on the CPU and upload it into one
//! fixed buffer at offset 0 when the phase flushes. The preview renderer
//! draws a whole path in a single phase, so a path with more dabs than the
//! cap would trip `DabBatch::queue_dab`'s debug assert (and fail wgpu
//! validation in release) unless the stroke engine flushes and submits when
//! the queue reaches the cap.
//!
//! Uses the blocking `test_utils::readback_texture` helper, native only.

use darkly::brush::gpu_context::MAX_DABS_PER_PHASE;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::{
    nodes::brush_settings, pipeline::BrushPipelines, preview_renderer::BrushStrokePreviewRenderer,
    DAB_REFERENCE_SIZE,
};
use darkly::gpu::preview::PreviewBackdrop;
use darkly::gpu::test_utils::{readback_texture, test_device};

const SIDE: u32 = 512;
const MARGIN: f32 = 8.0;
const ROW_STEP: f32 = 12.0;
/// Channel value above which a pixel counts as ink (white dabs over an
/// opaque black backdrop).
const INK: u8 = 8;

/// A boustrophedon over the canvas, one point every 8 px, so the dab count
/// at 1 px spacing is roughly the path length in pixels.
fn zig_zag() -> Vec<PaintInformation> {
    let mut path = Vec::new();
    let (x0, x1) = (MARGIN, SIDE as f32 - MARGIN);
    let mut y = MARGIN;
    let mut forward = true;
    while y <= SIDE as f32 - MARGIN {
        let mut x = if forward { x0 } else { x1 };
        loop {
            path.push(PaintInformation {
                pos: [x, y],
                pressure: 1.0,
                time: path.len() as f32 * 0.004,
                ..Default::default()
            });
            let next = if forward { x + 8.0 } else { x - 8.0 };
            if next < x0 || next > x1 {
                break;
            }
            x = next;
        }
        forward = !forward;
        y += ROW_STEP;
    }
    path
}

fn path_length(path: &[PaintInformation]) -> f32 {
    path.windows(2)
        .map(|w| (w[1].pos[0] - w[0].pos[0]).hypot(w[1].pos[1] - w[0].pos[1]))
        .sum()
}

#[test]
fn preview_render_splits_phases_at_the_dab_cap() {
    let mut graph = darkly::brush::default_graph();
    let settings = brush_settings::node_id(&graph).expect("default graph has brush_settings");
    // A 2 px wide tip at the 1 px spacing floor: one dab per pixel of path.
    graph
        .set_port_default(&settings, "size", 2.0 / DAB_REFERENCE_SIZE as f32)
        .unwrap();
    graph.set_port_default(&settings, "spacing", 0.0).unwrap();

    let path = zig_zag();
    assert!(
        path_length(&path) > 1.2 * MAX_DABS_PER_PHASE as f32,
        "the path must place more dabs than one phase holds"
    );

    let (device, queue) = test_device();
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut renderer = BrushStrokePreviewRenderer::new();
    let texture = renderer
        .render_stroke(
            &device,
            &queue,
            &pipelines,
            &graph,
            &path,
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            PreviewBackdrop::Flat,
            SIDE,
            SIDE,
            None,
        )
        .expect("render_stroke should return a texture");
    let pixels = readback_texture(
        &device,
        &queue,
        texture,
        wgpu::TextureFormat::Rgba8Unorm,
        SIDE,
        SIDE,
    );

    // Ink on the first row and the last row: dabs before and after every
    // phase split landed.
    let row_has_ink = |y: f32| {
        let y = y.round() as u32;
        (0..SIDE).any(|x| pixels[((y * SIDE + x) * 4) as usize] > INK)
    };
    let last_row = path.last().unwrap().pos[1];
    assert!(row_has_ink(MARGIN), "the first row is missing");
    assert!(row_has_ink(last_row), "the last row is missing");
}
