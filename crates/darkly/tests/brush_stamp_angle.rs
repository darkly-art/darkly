//! Native-only integration test for the stamp turn-rate limit.
//!
//! The unit tests in `stroke_engine.rs` pin the tracker's math against the
//! free function directly. This one proves the *wiring*: that the
//! `brush_settings.stamp_angle_rate` port is read at stroke start, reaches the
//! tracker, lands back on the dab's drawing angle, and rotates the stamp the
//! shader draws. Delete any link in that chain and every unit test still
//! passes while the feature is silently gone.
//!
//! The stroke-start tests below pin where a dab's direction comes from: the
//! chord from the dab before it, and for a stroke's first dab the hover the
//! pen arrived from, so the stroke continues the hover stream instead of
//! starting from a placeholder.
//!
//! Uses the blocking `test_utils::readback_texture` helper, native only.

mod common;

use common::{new_engine, readback, set_input, Vehicle};
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
/// 180° reversal a no-op at *any* rate (that is the whole point of it), so a
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
/// follows the stroke, so the stamp's angle is visible in the pixels, and
/// the given turn rate.
fn graph_with_rate(rate: f32) -> darkly::nodegraph::Graph<darkly::brush::wire::BrushWireType> {
    graph_with(rate, 0.35, None)
}

/// [`graph_with_rate`] at the given base `size` and, if set, dab `spacing`.
fn graph_with(
    rate: f32,
    size: f32,
    spacing: Option<f32>,
) -> darkly::nodegraph::Graph<darkly::brush::wire::BrushWireType> {
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
    graph.set_port_default(&settings, "size", size).unwrap();
    if let Some(spacing) = spacing {
        graph
            .set_port_default(&settings, "spacing", spacing)
            .unwrap();
    }
    graph
        .set_port_default(&settings, "stamp_angle_rate", rate)
        .unwrap();

    graph
}

fn render(rate: f32) -> Vec<u8> {
    render_path(&graph_with_rate(rate), &corner_path())
}

fn render_path(
    graph: &darkly::nodegraph::Graph<darkly::brush::wire::BrushWireType>,
    path: &[PaintInformation],
) -> Vec<u8> {
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
            graph,
            path,
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
/// corner: the stamp lags into the turn instead of snapping through it.
#[test]
fn turn_rate_changes_the_painted_corner() {
    let free = render(brush_settings::STAMP_ANGLE_RATE_UNLIMITED);
    // 20°/width: at the default 10% spacing, 2° per dab, so a 90° corner then
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
         rate never reached the tracker: check that \
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

/// The preview seed is fixed, so two renders of the same graph are identical,
/// without which the comparisons above could be measuring dab jitter.
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

/// Pixels whose red channel differs by more than 24 between two renders.
fn differing(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(a, b)| (a[0] as i16 - b[0] as i16).abs() > 24)
        .count()
}

/// Regression: a stroke's first point has no direction behind it, and the
/// placeholder 0 it carried was blended into every dab of the first segment,
/// so the stamp started facing canvas-right and the turn rate then crawled it
/// round: the twist at the start of every Flat Pen stroke. A straight stroke
/// never turns, so a locked stamp and a free one must paint it alike.
///
/// The corner tests above could not see it: their path starts along +x,
/// where 0 happens to be right, and at their size the spacing step is longer
/// than a segment, so no dab lands inside the first one. Flat Pen's tight
/// spacing on a smaller tip puts several there.
#[test]
fn straight_stroke_paints_the_same_at_any_turn_rate() {
    let path: Vec<PaintInformation> = (0..40)
        .map(|i| sample([80.0, 140.0 - i as f32 * 3.0], i))
        .collect();
    let locked = render_path(&graph_with(0.0, 0.1, Some(0.02)), &path);
    let free = render_path(
        &graph_with(brush_settings::STAMP_ANGLE_RATE_UNLIMITED, 0.1, Some(0.02)),
        &path,
    );

    let painted = free.as_chunks::<4>().0.iter().filter(|p| p[0] > 24).count();
    assert!(painted > 0, "the free render painted nothing");
    let differ = differing(&locked, &free);
    assert!(
        differ < painted / 50,
        "a straight stroke must paint the same at any turn rate: {differ} of \
         {painted} painted px differ, so the stamp started facing somewhere \
         other than the stroke's own direction"
    );
}

const CANVAS: (u32, u32) = (200, 200);

/// Flat Pen down a straight vertical line, rendering every third event so the
/// stabilizer's rewinds run near the start, at its own turn rate or `rate`.
fn flat_pen_vertical(rate: Option<f32>) -> (Vec<u8>, darkly::coord::CanvasRect) {
    let mut engine = new_engine(CANVAS, Vehicle::Builtin("Flat Pen"), 0.5);
    engine.set_stroke_seed(Some(7));
    if let Some(rate) = rate {
        set_input(&mut engine, "brush_settings", "stamp_angle_rate", rate);
    }
    let layer = engine.add_raster_layer(None);
    engine.begin_stroke(layer).unwrap();
    for i in 0..60 {
        engine.stroke_to(common::event(100.0, 170.0 - 2.0 * i as f32, i as f64 * 8.0));
        if i % 3 == 2 {
            engine.render(0.0);
        }
    }
    engine.end_stroke();
    readback(&mut engine, layer)
}

/// Regression, through the engine with the shipped brush: its stabilizer,
/// spacing and turn rate, with rewinds mid-stroke. The turn rate is the only
/// difference between the two renders and a straight stroke never turns.
#[test]
fn flat_pen_straight_stroke_does_not_twist_at_the_start() {
    let own = flat_pen_vertical(None);
    let free = flat_pen_vertical(Some(brush_settings::STAMP_ANGLE_RATE_UNLIMITED));
    common::assert_same(
        "straight vertical Flat Pen stroke",
        "own turn rate",
        own,
        "unlimited turn rate",
        free,
    );
}

/// A hover pose at `pos`, as the cursor preview receives it.
fn hover(engine: &mut darkly::engine::DarklyEngine, pos: [f32; 2]) {
    engine.regenerate_brush_cursor_preview_with_pen(PaintInformation {
        pos,
        pressure: 1.0,
        ..Default::default()
    });
}

/// Three hover poses straight down onto `to`.
fn hover_down_onto(engine: &mut darkly::engine::DarklyEngine, to: [f32; 2]) {
    for dy in [-8.0, -4.0, 0.0] {
        hover(engine, [to[0], to[1] + dy]);
    }
}

/// An engine painting with the slit graph at turn `rate`, and its layer.
fn slit_engine(rate: f32) -> (darkly::engine::DarklyEngine, darkly::layer::LayerId) {
    let mut engine = new_engine(CANVAS, Vehicle::Disc, 0.0);
    let json = serde_json::to_string(&graph_with(rate, 0.12, Some(0.05))).unwrap();
    engine.set_brush_graph(&json).unwrap();
    let layer = engine.add_raster_layer(None);
    (engine, layer)
}

fn stroke(
    engine: &mut darkly::engine::DarklyEngine,
    layer: darkly::layer::LayerId,
    pts: &[[f32; 2]],
) {
    let ops: Vec<_> = pts
        .iter()
        .enumerate()
        .map(|(i, p)| common::event(p[0], p[1], i as f64 * 8.0))
        .collect();
    common::headless(engine, layer, &ops);
}

/// Painted bounding box `(x0, y0, x1, y1)` inside `area` (same form).
fn painted_bbox(
    (px, bounds): &(Vec<u8>, darkly::coord::CanvasRect),
    area: (i32, i32, i32, i32),
) -> Option<(i32, i32, i32, i32)> {
    let mut bbox: Option<(i32, i32, i32, i32)> = None;
    for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
        if p[3] <= 24 {
            continue;
        }
        let x = bounds.origin.x + (i as u32 % bounds.width) as i32;
        let y = bounds.origin.y + (i as u32 / bounds.width) as i32;
        if x < area.0 || y < area.1 || x > area.2 || y > area.3 {
            continue;
        }
        bbox = Some(match bbox {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    bbox
}

fn aspect((x0, y0, x1, y1): (i32, i32, i32, i32)) -> f32 {
    (x1 - x0 + 1) as f32 / (y1 - y0 + 1) as f32
}

/// A tap after a hover faces the way the pen arrived, as the hover preview
/// showed it; the pose is consumed, so a second tap with no hover between
/// faces the authored orientation. Compared against each other so the test
/// does not depend on which axis the slit's long side lies on at rotation 0.
#[test]
fn a_tap_continues_the_hover_direction() {
    let (mut engine, layer) = slit_engine(0.0);
    hover_down_onto(&mut engine, [60.0, 100.0]);
    stroke(&mut engine, layer, &[[60.0, 100.0]]);
    stroke(&mut engine, layer, &[[140.0, 100.0]]);
    let pixels = readback(&mut engine, layer);

    let after_hover = painted_bbox(&pixels, (0, 0, 99, 199)).expect("first tap painted");
    let no_hover = painted_bbox(&pixels, (100, 0, 199, 199)).expect("second tap painted");
    let (a, b) = (aspect(after_hover), aspect(no_hover));
    assert!(
        (a - 1.0) * (b - 1.0) < 0.0,
        "the tap after a downward hover must face a quarter turn from the tap \
         with no hover: aspects {a:.2} and {b:.2}"
    );
}

/// A stroke that continues straight on from its hover is uniform from its
/// first dab: the start dab faces the hover direction, and the dab the first
/// segment lays on the start point again falls back to that direction rather
/// than to a placeholder. A dab facing along the stroke reaches past the first
/// point by half the slit's thickness; one facing elsewhere reaches up to half
/// its length, which is the stroke's width.
#[test]
fn a_stroke_continuing_its_hover_is_uniform_from_the_start() {
    let (mut engine, layer) = slit_engine(brush_settings::STAMP_ANGLE_RATE_UNLIMITED);
    let pts: Vec<[f32; 2]> = (0..50).map(|i| [100.0, 50.0 + 2.0 * i as f32]).collect();
    hover_down_onto(&mut engine, pts[0]);
    stroke(&mut engine, layer, &pts);
    let pixels = readback(&mut engine, layer);

    let (_, y0, _, _) = painted_bbox(&pixels, (0, 0, 199, 199)).expect("stroke painted");
    let (x0, _, x1, _) = painted_bbox(&pixels, (0, 90, 199, 110)).expect("middle painted");
    let overhang = pts[0][1] - y0 as f32;
    let width = (x1 - x0 + 1) as f32;
    assert!(
        overhang < width / 4.0,
        "the paint reaches {overhang} px past the first point of a stroke {width} px \
         wide: a start dab faced away from the stroke"
    );
}

/// The hover direction reaches only the start dab: a locked stamp takes its
/// orientation from the first dab that travels, so a stroke heading off at a
/// right angle to its hover paints, away from the start, exactly as with no
/// hover at all.
#[test]
fn the_hover_direction_does_not_lock_the_stamp() {
    let pts: Vec<[f32; 2]> = (0..40).map(|i| [40.0 + 3.0 * i as f32, 100.0]).collect();

    let (mut hovered, layer) = slit_engine(0.0);
    hover_down_onto(&mut hovered, pts[0]);
    stroke(&mut hovered, layer, &pts);
    let hovered = readback(&mut hovered, layer);

    let (mut plain, layer) = slit_engine(0.0);
    stroke(&mut plain, layer, &pts);
    let plain = readback(&mut plain, layer);

    let away = (80, 0, 199, 199);
    let crop = |(px, bounds): &(Vec<u8>, darkly::coord::CanvasRect)| -> Vec<u8> {
        px.as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| bounds.origin.x + (*i as u32 % bounds.width) as i32 >= away.0)
            .flat_map(|(_, p)| *p)
            .collect()
    };
    assert_eq!(
        hovered.1, plain.1,
        "the two strokes grew the layer differently"
    );
    assert!(
        crop(&hovered) == crop(&plain),
        "past its start, a stroke must paint the same whatever its hover"
    );
}
