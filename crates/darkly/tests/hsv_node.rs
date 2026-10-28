//! The `hsv` node: colour in, hue / saturation / value deltas in, colour out.
//!
//! Pins the CPU path against `darkly::color` (the function the global colour
//! jitter also calls), the natural-range contract on its inputs (`saturation`
//! centers a `[0, 1]` source, `hue` takes degrees raw), that the compiled
//! WGSL calls the shared `hsv_adjust` lib and binds its input once, and that
//! the GPU path lands on the CPU result.

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::portable::PortableBrush;
use darkly::brush::wire::{BrushWireType, ScalarValue};
use darkly::brush::{compile_graph, registry};
use darkly::color::hsv_adjust_rgba;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{Graph, NodeId, PortRef};

const COLOR: [f32; 4] = [0.8, 0.2, 0.1, 1.0];
const HUE: f32 = 90.0;
const SATURATION: f32 = -0.3;
const VALUE: f32 = 0.2;

fn wire(graph: &mut Graph<BrushWireType>, from: (&NodeId, &str), to: (&NodeId, &str)) {
    graph
        .connect(
            PortRef {
                node: from.0.clone(),
                port: from.1.into(),
            },
            PortRef {
                node: to.0.clone(),
                port: to.1.into(),
            },
        )
        .expect("wire connects");
}

/// Evaluate one dab with `COLOR` as the paint colour.
fn run(graph: &Graph<BrushWireType>) -> BrushGraphRunner {
    let registry = registry();
    let mut runner =
        BrushGraphRunner::new(graph, registry.as_map(), registry.evaluators()).unwrap();
    runner.seed_sensors(&PaintInformation::default(), COLOR, 42, 0);
    runner.execute_cpu();
    runner
}

fn read(runner: &BrushGraphRunner, type_id: &str, port: &str) -> ScalarValue {
    let slot = runner
        .find_output_slot(type_id, port)
        .unwrap_or_else(|| panic!("{type_id}.{port} has a slot"));
    runner.read_slot(slot).expect("slot has a value")
}

fn read_color(runner: &BrushGraphRunner, type_id: &str, port: &str) -> [f32; 4] {
    match read(runner, type_id, port) {
        ScalarValue::Vec4(v) => v,
        other => panic!("expected Vec4, got {other:?}"),
    }
}

fn read_scalar(runner: &BrushGraphRunner, type_id: &str, port: &str) -> f32 {
    match read(runner, type_id, port) {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    }
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

/// `paint_color -> hsv` plus whatever `extra` adds, with `extra` run before
/// the wires so it can add nodes.
fn graph_with(extra: impl FnOnce(&mut Graph<BrushWireType>, &NodeId)) -> Graph<BrushWireType> {
    let registry = registry();
    let mut graph = Graph::new();
    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
    );
    let hsv = graph.add_node("hsv", registry.get("hsv").unwrap().ports.clone());
    wire(&mut graph, (&paint_color, "color"), (&hsv, "color"));
    extra(&mut graph, &hsv);
    graph
}

#[test]
fn cpu_matches_color_module() {
    let graph = graph_with(|graph, hsv| {
        graph.set_port_default(hsv, "hue", HUE).unwrap();
        graph
            .set_port_default(hsv, "saturation", SATURATION)
            .unwrap();
        graph.set_port_default(hsv, "value", VALUE).unwrap();
    });
    let runner = run(&graph);
    assert_eq!(
        read_color(&runner, "hsv", "output"),
        hsv_adjust_rgba(COLOR, HUE, SATURATION, VALUE),
        "the node is the shared transform, nothing more"
    );
}

#[test]
fn defaults_pass_the_colour_through_bit_identically() {
    let runner = run(&graph_with(|_, _| {}));
    assert_eq!(read_color(&runner, "hsv", "output"), COLOR);
}

/// `saturation` declares `[-1, 1]`, so `random`'s `[0, 1)` lands centered.
#[test]
fn random_into_saturation_is_bipolar() {
    let graph = graph_with(|graph, hsv| {
        let random = graph.add_node("random", registry().get("random").unwrap().ports.clone());
        wire(graph, (&random, "value"), (hsv, "saturation"));
    });
    let runner = run(&graph);
    let r = read_scalar(&runner, "random", "value");
    assert!((0.0..1.0).contains(&r));
    let got = read_color(&runner, "hsv", "output");
    let want = hsv_adjust_rgba(COLOR, 0.0, -1.0 + 2.0 * r, 0.0);
    assert!(close(got, want), "got {got:?}, want {want:?} for r = {r}");
}

/// `hue` declares `[-180, 180]`, so `random`'s `[0, 1)` spans a full turn
/// centered on "no change".
#[test]
fn random_into_hue_spans_a_full_turn() {
    let graph = graph_with(|graph, hsv| {
        let random = graph.add_node("random", registry().get("random").unwrap().ports.clone());
        wire(graph, (&random, "value"), (hsv, "hue"));
    });
    let runner = run(&graph);
    let r = read_scalar(&runner, "random", "value");
    let got = read_color(&runner, "hsv", "output");
    let want = hsv_adjust_rgba(COLOR, -180.0 + 360.0 * r, 0.0, 0.0);
    assert!(close(got, want), "got {got:?}, want {want:?} for r = {r}");
}

/// Ink with an `hsv` node spliced between `paint_color` and `stamp`.
fn hsv_brush_yaml(consumed: bool) -> String {
    let color_wires = if consumed {
        "- paint_color.color -> hsv.color\n- hsv.output -> stamp.color\n"
    } else {
        "- paint_color.color -> hsv.color\n- paint_color.color -> stamp.color\n"
    };
    format!(
        "name: HSV Test
description: Ink through an HSV Adjust.
nodes:
  brush_settings:
    type: brush_settings
  circle:
    type: circle
    inputs:
      softness: 0.1
  hsv:
    type: hsv
    inputs:
      hue: {HUE}
      saturation: {SATURATION}
      value: {VALUE}
  paint:
    type: paint
  paint_color:
    type: paint_color
  pen_input:
    type: pen_input
  stamp:
    type: stamp
connections:
- circle.mask -> stamp.tip
{color_wires}- pen_input.position -> paint.position
- pen_input.pressure -> paint.build_flow
- pen_input.pressure -> paint.size
- stamp.dab -> paint.rgba
"
    )
}

fn parse(yaml: &str) -> Graph<BrushWireType> {
    let portable: PortableBrush = serde_yaml_ng::from_str(yaml).expect("yaml parses");
    portable.into_graph(registry()).expect("graph builds")
}

fn naga_validate(label: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{label}: {}\n{source}", e.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{label}: {e:?}\n{source}"));
}

#[test]
fn wgsl_validates_and_calls_shared_lib() {
    let runner = compile_graph(&parse(&hsv_brush_yaml(true))).expect("compiles");
    let compiled = runner.compiled_brush().expect("has a terminal");
    for (variant, source) in [
        ("stroke", &compiled.stroke_wgsl),
        ("cursor_preview", &compiled.cursor_preview_wgsl),
    ] {
        naga_validate(variant, source);
    }
    let stroke = &compiled.stroke_wgsl;
    assert!(
        stroke.contains("fn hsv_adjust("),
        "the shared lib is in the prelude"
    );
    assert_eq!(
        stroke.matches("let hsv_c_hsv =").count(),
        1,
        "the input is bound exactly once"
    );
    assert!(stroke.contains("hsv_adjust(hsv_c_hsv.rgb,"));

    let runner = compile_graph(&parse(&hsv_brush_yaml(false))).expect("compiles");
    let compiled = runner.compiled_brush().expect("has a terminal");
    assert!(
        !compiled.stroke_wgsl.contains("hsv_c_"),
        "an unconsumed node emits nothing"
    );
}

/// The GPU path (compiled WGSL through a real stroke) lands on the CPU
/// transform, within 8-bit readback tolerance.
#[test]
fn gpu_matches_cpu() {
    let (w, h) = (128u32, 64u32);
    let (device, queue) = test_device();
    let mut engine = DarklyEngine::new(GpuContext::new_headless(device, queue), w, h);
    engine
        .set_brush_graph_yaml(&hsv_brush_yaml(true))
        .expect("brush loads");
    let layer = engine.add_raster_layer(None);
    engine.set_foreground_color([COLOR[0], COLOR[1], COLOR[2], COLOR[3]]);
    engine.begin_stroke(layer).unwrap();
    for i in 0..20 {
        engine.stroke_to(StrokeOp::BrushStroke {
            x: 8.0 + i as f32 * ((w - 16) as f32 / 19.0),
            y: (h / 2) as f32,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
        });
    }
    engine.end_stroke();
    engine.render(0.0);

    let pixels = engine.test_readback_layer(layer);
    let i = (((h / 2) * w + w / 2) * 4) as usize;
    let px = &pixels[i..i + 4];
    assert!(px[3] >= 250, "centre is opaque, got alpha {}", px[3]);
    let want = hsv_adjust_rgba(COLOR, HUE, SATURATION, VALUE);
    for c in 0..3 {
        let got = px[c] as f32 / px[3] as f32;
        assert!(
            (got - want[c]).abs() <= 3.0 / 255.0,
            "channel {c}: gpu {got} vs cpu {} (pixel {px:?}, want {want:?})",
            want[c]
        );
    }
}
