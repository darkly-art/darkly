//! Document DPI: the document-side scalar, its undo and persistence coupling,
//! and the `document_settings` node that delivers it to a brush.
//!
//! Run with:
//! `cargo test -p darkly --test document_dpi --features testing -- --test-threads=1`

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::registry;
use darkly::brush::wire::ScalarValue;
use darkly::document::DEFAULT_DPI;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{compile, Graph, NodeId, PortRef};

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn wire(
    graph: &mut Graph<darkly::brush::wire::BrushWireType>,
    from: (&NodeId, &str),
    to: (&NodeId, &str),
) {
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
        .expect("ports connect");
}

/// Slot index a specific node instance writes `port` to. `find_output_slot`
/// resolves by type id, which is ambiguous in a graph with two `multiply`
/// nodes; the plan carries the node id.
fn slot_of(graph: &Graph<darkly::brush::wire::BrushWireType>, node_id: &str, port: &str) -> usize {
    let plan = compile(graph, registry().as_map()).expect("graph compiles");
    plan.steps
        .iter()
        .find(|s| s.node_id.0 == node_id)
        .unwrap_or_else(|| panic!("no node {node_id} in plan"))
        .output_slots
        .iter()
        .find(|(name, _)| name == port)
        .unwrap_or_else(|| panic!("node {node_id} has no output {port}"))
        .1
}

// ---------------------------------------------------------------------------
// The document field
// ---------------------------------------------------------------------------

/// A fresh document reports the configured default, and the setter refuses
/// anything outside the band (or non-finite) rather than clamping it, leaving
/// the document untouched and pushing no undo entry.
#[test]
fn dpi_defaults_and_rejects_out_of_range() {
    let mut engine = test_engine(64, 64);
    assert_eq!(engine.document_dpi(), DEFAULT_DPI);
    assert!(!engine.is_dirty());

    for bad in [0.0, 0.5, 1e9, f32::NAN, f32::INFINITY, -300.0] {
        engine.set_document_dpi(bad);
        assert_eq!(
            engine.document_dpi(),
            DEFAULT_DPI,
            "set_document_dpi({bad}) must be rejected, not clamped"
        );
        assert!(
            !engine.is_dirty(),
            "a rejected set_document_dpi({bad}) must push no undo entry"
        );
    }

    engine.set_document_dpi(72.5);
    assert_eq!(engine.document_dpi(), 72.5);
}

/// A DPI change is undoable, and being undoable is what marks the document
/// dirty: without it, closing the tab would silently discard the change.
/// Rendering after the undo shows the action needs no compositor reconcile
/// (unlike a canvas resize, DPI changes no window-sized GPU resource).
#[test]
fn set_dpi_is_undoable_and_marks_dirty() {
    let mut engine = test_engine(64, 64);
    engine.set_document_dpi(600.0);
    assert_eq!(engine.document_dpi(), 600.0);
    assert!(engine.is_dirty());

    engine.undo();
    assert_eq!(engine.document_dpi(), DEFAULT_DPI);
    engine.render(0.0);
    assert_eq!(engine.canvas_dimensions(), (64, 64));

    engine.redo();
    assert_eq!(engine.document_dpi(), 600.0);
    engine.render(0.0);
}

/// Image Size resamples the artwork without changing how big it is, so the
/// resolution rides the factor and the document's physical extent is
/// preserved: 1024 px at 300 DPI taken to 4096 px is 4096 px at 1200 DPI,
/// both 3.41 inches wide. Size and resolution undo as one step, which is what
/// makes them one action rather than two.
#[test]
fn rescale_image_preserves_physical_extent() {
    let mut engine = test_engine(128, 128);
    let _layer = engine.add_raster_layer(None);
    assert_eq!(engine.document_dpi(), DEFAULT_DPI);

    engine.rescale_image(512, 512);
    assert_eq!(engine.canvas_dimensions(), (512, 512));
    assert_eq!(engine.document_dpi(), DEFAULT_DPI * 4.0);

    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (128, 128));
    assert_eq!(
        engine.document_dpi(),
        DEFAULT_DPI,
        "dims and resolution must ride one undo step"
    );

    engine.redo();
    assert_eq!(engine.canvas_dimensions(), (512, 512));
    assert_eq!(engine.document_dpi(), DEFAULT_DPI * 4.0);
}

// ---------------------------------------------------------------------------
// The node
// ---------------------------------------------------------------------------

/// `document_settings.dpi_scale` is the document's resolution over the
/// reference, published on a plain output slot like any other CPU node: 1.0 at
/// the default DPI, 2.0 at twice it.
#[test]
fn document_settings_publishes_dpi_scale() {
    let eval_at = |dpi: Option<f32>| -> f32 {
        let reg = registry();
        let mut graph = Graph::new();
        let ds = graph.add_node(
            "document_settings",
            reg.get("document_settings").unwrap().ports.clone(),
        );
        let mul = graph.add_node("multiply", reg.get("multiply").unwrap().ports.clone());
        graph.set_port_default(&mul, "b", 1.0).unwrap();
        wire(&mut graph, (&ds, "dpi_scale"), (&mul, "a"));

        let mut runner = BrushGraphRunner::new(&graph, reg.as_map(), reg.evaluators()).unwrap();
        if let Some(dpi) = dpi {
            runner.set_dpi(dpi);
        }
        runner.seed_sensors(&PaintInformation::default(), [0.0, 0.0, 0.0, 1.0], 42, 0);
        runner.execute_cpu();
        let slot = runner.find_output_slot("multiply", "result").unwrap();
        match runner.read_slot(slot).expect("result has value") {
            ScalarValue::Scalar(v) => v,
            other => panic!("expected Scalar, got {other:?}"),
        }
    };

    // A runner nobody seeded (the brush editor preview, a bare test) sits at
    // the reference, so the node is an identity there.
    assert!((eval_at(None) - 1.0).abs() < 1e-6);
    assert!((eval_at(Some(DEFAULT_DPI)) - 1.0).abs() < 1e-6);
    assert!((eval_at(Some(DEFAULT_DPI * 2.0)) - 2.0).abs() < 1e-6);
    assert!((eval_at(Some(DEFAULT_DPI / 4.0)) - 0.25).abs() < 1e-6);
}

/// The value reaches the shader as a stroke-constant uniform, not as a literal
/// baked into the WGSL at compile time: a compiled graph is document-agnostic
/// and must not need recompiling when the artist changes the DPI.
#[test]
fn dpi_scale_is_a_uniform_not_a_baked_literal() {
    let charcoal = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Charcoal")
        .expect("Charcoal brush registered");
    let graph = &charcoal.metadata.graph;
    let node_id = graph
        .nodes()
        .iter()
        .find(|(_, n)| n.type_id == "document_settings")
        .map(|(id, _)| id.clone())
        .expect("Charcoal wires document_settings");

    // Through `compile_graph`, the same entry point the engine uses, so this
    // is the shader the shipped brush actually paints with.
    let compiled = darkly::brush::compile_graph(graph)
        .expect("Charcoal compiles")
        .compiled_brush()
        .expect("Charcoal terminates in `paint`, so it compiles to WGSL");

    // `CompileWgslCtx::uniform_field_name` names node contributions
    // `n{node_id}_{port}`; the packer finds the value under the same key that
    // `build_slot_outputs` published it under.
    let field = format!("n{}_dpi_scale", node_id.0);
    assert!(
        compiled.uniform_layout.iter().any(|f| f.name == field),
        "expected a {field} uniform field, got {:?}",
        compiled
            .uniform_layout
            .iter()
            .map(|f| &f.name)
            .collect::<Vec<_>>()
    );
    assert!(
        compiled.stroke_wgsl.contains(&format!("u.{field}")),
        "the shader must read {field} out of the uniform buffer"
    );
}

/// Charcoal's paper texture is wired through `dpi_scale`, and at the default
/// DPI that wiring is an identity: the scale arriving at the image node is the
/// 500 px the brush authored. This is what makes the wiring safe to ship, and
/// it fails if `DEFAULT_DPI` and the authored operand ever disagree.
#[test]
fn charcoal_paper_scale_is_unchanged_at_the_default_dpi() {
    let charcoal = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Charcoal")
        .expect("Charcoal brush registered");
    let graph = &charcoal.metadata.graph;
    let reg = registry();

    let mut runner = BrushGraphRunner::new(graph, reg.as_map(), reg.evaluators()).unwrap();
    runner.set_dpi(DEFAULT_DPI);
    runner.seed_sensors(&PaintInformation::default(), [0.0, 0.0, 0.0, 1.0], 7, 0);
    runner.execute_cpu();

    let slot = slot_of(graph, "multiply_2", "result");
    let scale = match runner.read_slot(slot).expect("paper scale computed") {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    };
    assert!(
        (scale - 500.0).abs() < 1e-3,
        "Charcoal's paper scale must be its authored 500 px at the default DPI, got {scale}"
    );
}

// ---------------------------------------------------------------------------
// Document to shader, end to end
// ---------------------------------------------------------------------------

/// The plumb the node is useless without: `Document::dpi` reaches the graph
/// through `StrokeEngine::new`. Wire `dpi_scale` into the terminal's per-touch
/// size multiplier and paint one dab; halving the document's resolution must
/// halve the painted mark. Fails if `StrokeEngine::new` stops forwarding DPI.
#[test]
fn dpi_reaches_the_graph_from_the_document() {
    let painted_width_at = |dpi: f32| -> u32 {
        let (w, h) = (256u32, 256u32);
        let mut engine = test_engine(w, h);
        let layer = engine.add_raster_layer(None);
        engine.set_document_dpi(dpi);

        let reg = registry();
        let mut graph = engine.default_brush_graph();
        let ds = graph.add_node(
            "document_settings",
            reg.get("document_settings").unwrap().ports.clone(),
        );
        let terminal = darkly::brush::find_terminal(&graph).expect("default graph has a terminal");
        wire(&mut graph, (&ds, "dpi_scale"), (&terminal, "size"));
        engine
            .set_brush_graph(&serde_json::to_string(&graph).unwrap())
            .expect("graph compiles");

        engine.begin_stroke(layer).unwrap();
        for i in 0..2 {
            engine.stroke_to(StrokeOp::BrushStroke {
                x: 128.0,
                y: 128.0,
                pressure: 1.0,
                x_tilt: 0.0,
                y_tilt: 0.0,
                rotation: 0.0,
                tangential_pressure: 0.0,
                time_ms: i as f64 * 16.0,
                cr: 1.0,
                cg: 0.0,
                cb: 0.0,
                ca: 1.0,
            });
        }
        engine.end_stroke();

        let px = engine.test_readback_canvas();
        let mut min_x = u32::MAX;
        let mut max_x = 0u32;
        for y in 0..h {
            for x in 0..w {
                if px[((y * w + x) * 4 + 3) as usize] > 8 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        assert!(min_x <= max_x, "nothing was painted at {dpi} DPI");
        max_x - min_x + 1
    };

    let reference = painted_width_at(DEFAULT_DPI);
    let half = painted_width_at(DEFAULT_DPI / 2.0);
    assert!(
        reference > 8,
        "the reference dab is too small to measure ({reference} px)"
    );
    let ratio = half as f32 / reference as f32;
    assert!(
        (ratio - 0.5).abs() < 0.1,
        "halving the document's DPI must halve a dpi_scale-driven dab: \
         {reference} px at {DEFAULT_DPI} DPI vs {half} px at half that (ratio {ratio})"
    );
}
