//! The nominal dab radius must bound the dab footprint.
//!
//! `circle`'s `aspect` knob is a *contraction* of the silhouette: it narrows
//! the tip along one axis and never lengthens it along the other. So a dab's
//! ink stays inside the radius the brush's `size` port asks for, whatever
//! `aspect` is set to and whatever a wire delivers into it.
//!
//! These are rendered tests rather than extent-protocol assertions (those live
//! in `wgsl.rs`) because the bug they defend against was a disagreement
//! between the declared bound and the pixels: the extent said one radius and
//! the shader drew ten times further.
//!
//! Uses the blocking `test_utils::readback_texture` helper, native only.

use darkly::brush::paint_info::PaintInformation;
use darkly::brush::wire::BrushWireType;
use darkly::brush::{
    nodes::brush_settings, pipeline::BrushPipelines, preview_renderer::BrushStrokePreviewRenderer,
    DAB_REFERENCE_SIZE,
};
use darkly::gpu::preview::PreviewBackdrop;
use darkly::gpu::test_utils::{readback_texture, test_device};
use darkly::nodegraph::{Graph, NodeId, PortRef};

const SIDE: u32 = 512;
/// Nominal dab radius every case renders at. Large enough that a 100:1 nib
/// still has a measurable short axis, which a 16 px radius would not.
const RADIUS_PX: f32 = 100.0;
/// Channel value above which a pixel counts as ink. The preview renders a
/// white dab over an opaque black `PreviewBackdrop::Flat`, so ink is measured
/// against the backdrop's colour, not against alpha (every pixel is opaque).
const INK: u8 = 8;
/// Slack on every measured distance: the softness floor in `shape_coverage`
/// is a fraction of a pixel and the boundary is antialiased.
const SLACK: f32 = 1.5;

fn size_for(radius_px: f32) -> f32 {
    2.0 * radius_px / DAB_REFERENCE_SIZE as f32
}

fn find(graph: &Graph<BrushWireType>, type_id: &str) -> NodeId {
    graph
        .nodes()
        .iter()
        .find(|(_, n)| n.type_id == type_id)
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("graph has a {type_id} node"))
}

/// A single hard-edged dab at the canvas centre: `pen -> paint`,
/// `circle.mask -> stamp.tip`, `stamp.dab -> paint.rgba`. `rotation` is left
/// at 0 so the squashed axis lies along screen x, and `softness` at 0 so the
/// measured extent is the silhouette rather than the feather.
fn dab_graph(aspect: f32) -> Graph<BrushWireType> {
    let mut graph = darkly::brush::default_graph();
    let circle = find(&graph, "circle");
    graph.set_port_default(&circle, "aspect", aspect).unwrap();
    graph.set_port_default(&circle, "rotation", 0.0).unwrap();
    graph.set_port_default(&circle, "softness", 0.0).unwrap();
    let settings = brush_settings::node_id(&graph).expect("default graph has brush_settings");
    graph
        .set_port_default(&settings, "size", size_for(RADIUS_PX))
        .unwrap();
    graph
}

fn render(graph: &Graph<BrushWireType>) -> Vec<u8> {
    let (device, queue) = test_device();
    let pipelines = BrushPipelines::new(
        &device,
        &queue,
        &darkly::gpu::selection::selection_mask_bgl(&device),
    );
    let mut renderer = BrushStrokePreviewRenderer::new();
    let centre = PaintInformation {
        pos: [SIDE as f32 / 2.0, SIDE as f32 / 2.0],
        pressure: 1.0,
        time: 0.0,
        ..Default::default()
    };
    let texture = renderer
        .render_stroke(
            &device,
            &queue,
            &pipelines,
            graph,
            &[centre],
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            PreviewBackdrop::Flat,
            SIDE,
            SIDE,
            None,
        )
        .expect("render_stroke should return a texture");
    readback_texture(
        &device,
        &queue,
        texture,
        wgpu::TextureFormat::Rgba8Unorm,
        SIDE,
        SIDE,
    )
}

/// Every inked pixel's offset from the dab centre, in pixels.
fn ink_offsets(pixels: &[u8]) -> Vec<[f32; 2]> {
    let c = SIDE as f32 / 2.0;
    let mut out = Vec::new();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let lit = pixels[((y * SIDE + x) * 4) as usize];
            if lit > INK {
                out.push([x as f32 + 0.5 - c, y as f32 + 0.5 - c]);
            }
        }
    }
    out
}

/// Half-extent of the ink along each axis.
fn half_extents(offsets: &[[f32; 2]]) -> [f32; 2] {
    offsets.iter().fold([0.0_f32, 0.0_f32], |acc, o| {
        [acc[0].max(o[0].abs()), acc[1].max(o[1].abs())]
    })
}

/// A squashed nib's ink stays inside the nominal radius.
///
/// `aspect = 0.1` is authored rather than read from the port's range so the
/// assertion is stable across a change to that range. Before the fix the
/// silhouette reached `RADIUS_PX / 0.1`, ten times the nominal radius and
/// past the canvas edge.
#[test]
fn squashed_nib_ink_stays_inside_the_nominal_radius() {
    let pixels = render(&dab_graph(0.1));
    let offsets = ink_offsets(&pixels);
    assert!(!offsets.is_empty(), "the dab rendered nothing");

    let worst = offsets
        .iter()
        .fold(0.0_f32, |acc, o| acc.max(o[0].hypot(o[1])));
    assert!(
        worst <= RADIUS_PX + SLACK,
        "ink reaches {worst:.1} px from centre, outside the nominal radius {RADIUS_PX}"
    );

    // The squash is real, not a uniform shrink: the short axis is `aspect * R`.
    let [hx, hy] = half_extents(&offsets);
    assert!(
        hx <= 0.1 * RADIUS_PX + SLACK,
        "short axis reaches {hx:.1} px, expected at most {:.1}",
        0.1 * RADIUS_PX + SLACK
    );
    // Liveness: the long axis does reach the nominal radius, so the test
    // cannot pass by rendering a speck.
    assert!(
        hy >= 0.9 * RADIUS_PX,
        "long axis only reaches {hy:.1} px, expected about {RADIUS_PX}"
    );
}

/// The axis ratio is the `aspect` value. A fix that uniformly shrank the dab
/// would satisfy the containment test above but not this one.
#[test]
fn dab_axis_ratio_is_the_aspect_value() {
    let aspect = 0.25;
    let pixels = render(&dab_graph(aspect));
    let offsets = ink_offsets(&pixels);
    assert!(!offsets.is_empty(), "the dab rendered nothing");

    let [hx, hy] = half_extents(&offsets);
    assert!(
        (hy - RADIUS_PX).abs() <= SLACK,
        "long axis is {hy:.1} px, expected the nominal radius {RADIUS_PX}"
    );
    let ratio = hx / hy;
    assert!(
        (ratio - aspect).abs() <= 0.03,
        "axis ratio is {ratio:.3}, expected the authored aspect {aspect}"
    );
}

/// An `aspect` above 1 must not stretch the silhouette, whether it arrives as
/// an authored literal or through a wire.
///
/// The emitted expression clamps at both ends. Clamping only the low end (the
/// shape the naive contraction fix takes) lets a sensor wired into `aspect`
/// stretch the dab while the extent protocol still declares 1.0, which is the
/// bug being fixed, inverted.
#[test]
fn aspect_above_one_does_not_stretch_the_dab() {
    // Authored literal: the compile-time `InputBinding::Default` arm.
    let authored = render(&dab_graph(2.0));
    let worst = ink_offsets(&authored)
        .iter()
        .fold(0.0_f32, |acc, o| acc.max(o[0].hypot(o[1])));
    assert!(
        worst <= RADIUS_PX + SLACK,
        "authored aspect 2.0: ink reaches {worst:.1} px, outside the nominal radius"
    );

    // Through a wire: the `InputBinding::Wired` arm, where the value is only
    // known per fragment. `divide` with b = 0.2 delivers 5.0.
    let mut graph = dab_graph(1.0);
    let reg = darkly::brush::registry();
    let divide = graph.add_node("divide", reg.get("divide").unwrap().ports.clone());
    graph.set_port_default(&divide, "a", 1.0).unwrap();
    graph.set_port_default(&divide, "b", 0.2).unwrap();
    let circle = find(&graph, "circle");
    graph
        .connect(
            PortRef {
                node: divide,
                port: "result".into(),
            },
            PortRef {
                node: circle,
                port: "aspect".into(),
            },
        )
        .expect("divide.result -> circle.aspect");

    let wired = render(&graph);
    let worst = ink_offsets(&wired)
        .iter()
        .fold(0.0_f32, |acc, o| acc.max(o[0].hypot(o[1])));
    assert!(
        worst <= RADIUS_PX + SLACK,
        "wired aspect 5.0: ink reaches {worst:.1} px, outside the nominal radius"
    );
}
