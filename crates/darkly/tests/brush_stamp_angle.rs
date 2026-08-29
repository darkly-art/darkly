//! Native-only integration test for the stamp turn-rate limit.
//!
//! The unit tests in `stroke_engine.rs` pin the tracker's math against the
//! free function directly. This one proves the *wiring*: that the
//! `brush_settings.stamp_angle_rate` port is read at stroke start, reaches the
//! tracker, lands back on the dab's drawing angle, and rotates the stamp the
//! shader draws. Delete any link in that chain and every unit test still
//! passes while the feature is silently gone.
//!
//! Uses the blocking `test_utils::readback_texture` helper — native only.

use darkly::brush::paint_info::PaintInformation;
use darkly::brush::{
    default_graph, nodes::brush_settings, pipeline::BrushPipelines,
    preview_renderer::BrushStrokePreviewRenderer,
};
use darkly::gpu::preview::PreviewBackdrop;
use darkly::gpu::test_utils::{readback_texture, test_device};
use darkly::nodegraph::PortRef;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 160;

/// An L-shaped path: out along +x, then a hard 90° turn up along +y.
///
/// A 90° corner rather than a hairpin, deliberately. The axis fold makes a
/// 180° reversal a no-op at *any* rate — that is the whole point of it — so a
/// hairpin would render identically under both settings and prove nothing.
fn corner_path() -> Vec<PaintInformation> {
    let mut out = Vec::new();
    let corner = [110.0_f32, 80.0_f32];
    for i in 0..24 {
        out.push(sample([30.0 + i as f32 * (80.0 / 23.0), corner[1]], i));
    }
    for i in 1..24 {
        out.push(sample(
            [corner[0], corner[1] - i as f32 * (60.0 / 23.0)],
            23 + i,
        ));
    }
    out
}

fn sample(pos: [f32; 2], i: usize) -> PaintInformation {
    PaintInformation {
        pos,
        pressure: 1.0,
        time: i as f32 * 0.008,
        ..Default::default()
    }
}

/// The default brush, with a strongly anisotropic tip whose orientation
/// follows the stroke — so the stamp's angle is visible in the pixels — and
/// the given turn rate.
fn graph_with_rate(rate: f32) -> darkly::nodegraph::Graph<darkly::brush::wire::BrushWireType> {
    let mut graph = default_graph();

    let circle = graph
        .nodes()
        .iter()
        .find(|(_, n)| n.type_id == "circle")
        .map(|(id, _)| id.clone())
        .expect("default graph has a circle node");
    let pen = graph
        .nodes()
        .iter()
        .find(|(_, n)| n.type_id == "pen_input")
        .map(|(id, _)| id.clone())
        .expect("default graph has a pen_input node");

    // A slit rather than a disc: a disc is rotationally symmetric and would
    // render identically at every orientation.
    graph.set_port_default(&circle, "aspect", 0.12).unwrap();
    graph
        .connect(
            PortRef {
                node: pen,
                port: "drawing_angle".into(),
            },
            PortRef {
                node: circle.clone(),
                port: "rotation_input".into(),
            },
        )
        .expect("drawing_angle -> rotation_input");

    let settings = brush_settings::node_id(&graph).expect("default graph has brush_settings");
    graph.set_port_default(&settings, "size", 0.35).unwrap();
    graph
        .set_port_default(&settings, "stamp_angle_rate", rate)
        .unwrap();

    graph
}

fn render(rate: f32) -> Vec<u8> {
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
            &graph_with_rate(rate),
            &corner_path(),
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            PreviewBackdrop::Flat,
            WIDTH,
            HEIGHT,
            None,
        )
        .expect("render_stroke should return a texture");

    readback_texture(
        &device,
        &queue,
        texture,
        wgpu::TextureFormat::Rgba8Unorm,
        WIDTH,
        HEIGHT,
    )
}

/// Clamping the turn rate must visibly change what gets painted around a
/// corner — the stamp lags into the turn instead of snapping through it.
#[test]
fn turn_rate_changes_the_painted_corner() {
    let free = render(brush_settings::STAMP_ANGLE_RATE_UNLIMITED);
    // 20°/width: at the default 10% spacing, 2° per dab — a 90° corner then
    // takes tens of dabs to come around instead of one.
    let damped = render(20.0_f32.to_radians());

    assert_eq!(free.len(), damped.len());

    let differing = free
        .as_chunks::<4>()
        .0
        .iter()
        .zip(damped.as_chunks::<4>().0)
        .filter(|(a, b)| {
            // Compare luminance-ish: the stroke is white on black, so any
            // channel drifting is the stamp having covered different pixels.
            (a[0] as i16 - b[0] as i16).abs() > 24
        })
        .count();

    let painted = free.as_chunks::<4>().0.iter().filter(|p| p[0] > 24).count();
    assert!(painted > 0, "the unlimited render painted nothing at all");

    assert!(
        differing > painted / 20,
        "clamping the turn rate must change the painted corner: only \
         {differing} px differ against {painted} px painted. If this is 0, the \
         rate never reached the tracker — check that \
         `brush_settings.stamp_angle_rate` is read at stroke start and applied \
         in `place_dab`."
    );
}

/// A locked stamp (rate 0) holds the angle it started at for the whole
/// stroke, so the post-corner leg is painted with a stamp still oriented
/// along the *first* leg. Distinct from the test above: that one proves the
/// rate matters, this one proves which direction it biases.
#[test]
fn zero_rate_paints_the_whole_stroke_at_one_angle() {
    let locked = render(0.0);
    let free = render(brush_settings::STAMP_ANGLE_RATE_UNLIMITED);

    let count = |px: &[u8]| px.as_chunks::<4>().0.iter().filter(|p| p[0] > 24).count();
    assert!(count(&locked) > 0, "the locked render painted nothing");

    let differing = locked
        .as_chunks::<4>()
        .0
        .iter()
        .zip(free.as_chunks::<4>().0)
        .filter(|(a, b)| (a[0] as i16 - b[0] as i16).abs() > 24)
        .count();

    assert!(
        differing > count(&free) / 10,
        "a stamp locked at its starting angle must paint the vertical leg \
         differently from one that turned to follow it; only {differing} px \
         differ"
    );
}

/// The preview seed is fixed, so two renders of the same graph are identical
/// — without which the comparisons above could be measuring dab jitter.
#[test]
fn renders_are_deterministic() {
    let a = render(brush_settings::STAMP_ANGLE_RATE_UNLIMITED);
    let b = render(brush_settings::STAMP_ANGLE_RATE_UNLIMITED);
    assert!(
        a == b,
        "the same graph must render identically twice, or the turn-rate \
         comparisons are measuring noise"
    );
}
