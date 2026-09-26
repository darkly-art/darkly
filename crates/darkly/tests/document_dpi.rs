//! Document DPI: how a new document derives one, the DPI-only edit path and
//! its undo, and the reference-to-canvas boundary the whole feature exists
//! for (a brush keeping its physical mark size on any document).
//!
//! Run with:
//! `cargo test -p darkly --test document_dpi --features testing -- --test-threads=1`

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::registry;
use darkly::brush::wire::ScalarValue;
use darkly::document::{auto_dpi, SelectionMode, REFERENCE_DPI};
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{Graph, NodeId, PortRef};

/// A reference-DPI engine (`DarklyEngine::new`), so a reference pixel is a
/// canvas pixel and every pixel assertion below reads directly.
fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// An engine constructed the way a host makes a document: `None` derives the
/// DPI from the pixel size, `Some` installs the artist's value.
fn host_engine(width: u32, height: u32, dpi: Option<f32>) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::standalone(gpu, width, height, dpi)
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

/// The paper-grain shape the feature exists for: an authored feature size
/// driving `noise.scale`, whose field becomes the stamp's tip. Used to check
/// that the compiled shader reads the intrinsic rather than packing a
/// parallel uniform.
fn graph_with_dpi_scaled_grain() -> (Graph<darkly::brush::wire::BrushWireType>, NodeId) {
    let reg = registry();
    let mut graph = darkly::brush::default_graph();

    let ds = graph.add_node(
        "document_settings",
        reg.get("document_settings").unwrap().ports.clone(),
    );
    let mul = graph.add_node("multiply", reg.get("multiply").unwrap().ports.clone());
    graph.set_port_default(&mul, "a", 2.5).unwrap();
    let noise = graph.add_node("noise", reg.get("noise").unwrap().ports.clone());

    // The grain replaces the stock disc as the tip, so the stamp consumes it
    // and the compiler keeps the whole chain.
    let circle = graph
        .nodes()
        .iter()
        .find(|(_, n)| n.type_id == "circle")
        .map(|(id, _)| id.clone())
        .expect("the stock graph has a circle");
    let stamp = graph
        .nodes()
        .iter()
        .find(|(_, n)| n.type_id == "stamp")
        .map(|(id, _)| id.clone())
        .expect("the stock graph has a stamp");
    graph.disconnect(
        &PortRef {
            node: circle,
            port: "mask".into(),
        },
        &PortRef {
            node: stamp.clone(),
            port: "tip".into(),
        },
    );

    wire(&mut graph, (&ds, "dpi_scale"), (&mul, "b"));
    wire(&mut graph, (&mul, "result"), (&noise, "scale"));
    wire(&mut graph, (&noise, "value"), (&stamp, "tip"));
    (graph, ds)
}

// ---------------------------------------------------------------------------
// A document's DPI at birth
// ---------------------------------------------------------------------------

/// A host-made document derives its DPI from its pixel size unless the artist
/// names one, so every auto document has the reference's physical area and a
/// brush spans the same fraction of the artwork on all of them. An
/// out-of-band explicit value falls back to auto rather than installing a
/// degenerate DPI, and none of this is dirty or undoable: it is the
/// document's birth state, not an edit.
#[test]
fn new_document_dpi_is_auto_unless_stated() {
    // 2048 square is the largest the headless test device will allocate; it
    // derives 150, comfortably off the reference, so the assertions below
    // distinguish auto from both the reference and the manual value.
    let (w, h) = (2048u32, 2048u32);
    assert_eq!(auto_dpi(w, h), 150.0);

    let auto = host_engine(w, h, None);
    assert_eq!(auto.document_dpi(), 150.0);
    assert!(!auto.is_dirty());

    let manual = host_engine(w, h, Some(300.0));
    assert_eq!(manual.document_dpi(), 300.0);
    assert!(!manual.is_dirty());

    for bad in [f32::NAN, 0.0, -300.0, 1e9] {
        let fallback = host_engine(w, h, Some(bad));
        assert_eq!(
            fallback.document_dpi(),
            150.0,
            "an out-of-band {bad} must fall back to auto"
        );
    }

    // The test and tooling constructor pins the reference so pixel
    // expectations in every other suite read in canvas pixels.
    assert_eq!(test_engine(64, 64).document_dpi(), REFERENCE_DPI);
}

// ---------------------------------------------------------------------------
// rescale_image: the sole DPI mutator
// ---------------------------------------------------------------------------

/// Image Size resamples the artwork without changing how big it is, so with
/// no explicit DPI the value rides the factor and the document's physical
/// extent is preserved: 128 px at 100 DPI taken to 512 px is 512 px at 400,
/// both 1.28 inches wide. Dims and DPI undo as one step, which is what makes
/// them one action rather than two.
#[test]
fn rescale_image_derives_dpi_from_the_factor_by_default() {
    let mut engine = test_engine(128, 128);
    let _layer = engine.add_raster_layer(None);
    assert_eq!(engine.document_dpi(), REFERENCE_DPI);

    engine.rescale_image(512, 512, None);
    assert_eq!(engine.canvas_dimensions(), (512, 512));
    assert_eq!(engine.document_dpi(), REFERENCE_DPI * 4.0);

    engine.undo();
    assert_eq!(engine.canvas_dimensions(), (128, 128));
    assert_eq!(
        engine.document_dpi(),
        REFERENCE_DPI,
        "dims and DPI must ride one undo step"
    );

    engine.redo();
    assert_eq!(engine.canvas_dimensions(), (512, 512));
    assert_eq!(engine.document_dpi(), REFERENCE_DPI * 4.0);
}

/// An explicit DPI is the artist's answer to "how big is this artwork", so it
/// is installed as given and the resample factor does not touch it.
#[test]
fn rescale_image_installs_an_explicit_dpi() {
    let mut engine = test_engine(128, 128);
    let _layer = engine.add_raster_layer(None);

    engine.rescale_image(256, 256, Some(72.5));
    assert_eq!(engine.canvas_dimensions(), (256, 256));
    assert_eq!(engine.document_dpi(), 72.5);

    engine.undo();
    assert_eq!(engine.document_dpi(), REFERENCE_DPI);
}

/// Unchanged dimensions plus a DPI is the DPI-only edit: the document's
/// physical size changes and nothing else does. It must not resample, must
/// not clear the selection, and must not commit a floating selection, since
/// all three would be destructive side effects of changing one number. An
/// out-of-band explicit value is rejected outright rather than clamped, so a
/// bad caller leaves the document untouched and clean.
#[test]
fn rescale_image_with_unchanged_dims_is_a_dpi_only_edit() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);

    // Before any other edit, so "clean" is an honest reading of whether the
    // rejected calls pushed anything.
    for bad in [0.0, 0.5, 1e9, f32::NAN, f32::INFINITY, -300.0] {
        engine.rescale_image(w, h, Some(bad));
        assert_eq!(
            engine.document_dpi(),
            REFERENCE_DPI,
            "an explicit {bad} must be rejected, not clamped"
        );
        assert!(
            !engine.is_dirty(),
            "a rejected {bad} must push no undo entry"
        );
    }

    // The same DPI is no edit at all.
    engine.rescale_image(w, h, Some(REFERENCE_DPI));
    assert!(!engine.is_dirty(), "a no-op DPI must push no undo entry");

    let _layer = engine.add_raster_layer(None);
    engine.select_rect(8.0, 8.0, 16.0, 16.0, SelectionMode::Replace, false, 0.0);
    assert!(engine.has_selection());

    engine.rescale_image(w, h, Some(600.0));
    assert_eq!(engine.document_dpi(), 600.0);
    assert_eq!(engine.canvas_dimensions(), (w, h));
    assert!(
        engine.has_selection(),
        "a DPI-only edit resamples nothing, so it must keep the selection"
    );
    assert!(engine.is_dirty());

    engine.undo();
    assert_eq!(engine.document_dpi(), REFERENCE_DPI);
    assert!(engine.has_selection());
}

/// A DPI-only edit must leave an in-flight paste floating: committing it
/// would be a second, invisible edit riding on a change that touches no
/// pixels.
#[test]
fn a_dpi_only_edit_does_not_commit_a_floating_selection() {
    let (w, h) = (64u32, 64u32);
    let mut engine = test_engine(w, h);
    let layer = engine.add_raster_layer(None);
    engine.select_rect(8.0, 8.0, 32.0, 32.0, SelectionMode::Replace, false, 0.0);

    // With a selection the extraction bounds are known up front, so the
    // transform session starts immediately.
    engine.begin_transform(layer);
    assert!(engine.has_floating(), "setup: a float must be in flight");

    engine.rescale_image(w, h, Some(600.0));
    assert_eq!(engine.document_dpi(), 600.0);
    assert!(
        engine.has_floating(),
        "a DPI-only edit must not commit the floating selection"
    );
}

// ---------------------------------------------------------------------------
// The boundary
// ---------------------------------------------------------------------------

/// The reference-to-canvas factor is the document's DPI over the reference,
/// and `document_settings.dpi_scale` publishes exactly that: the same number
/// the runner applies at the boundary, not a second opinion about it.
#[test]
fn document_settings_publishes_the_intrinsic_factor() {
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
        let expected = runner.dpi_factor();
        runner.seed_sensors(&PaintInformation::default(), [0.0, 0.0, 0.0, 1.0], 42, 0);
        runner.execute_cpu();
        let slot = runner.find_output_slot("multiply", "result").unwrap();
        let published = match runner.read_slot(slot).expect("result has value") {
            ScalarValue::Scalar(v) => v,
            other => panic!("expected Scalar, got {other:?}"),
        };
        assert!(
            (published - expected).abs() < 1e-6,
            "dpi_scale ({published}) must be the runner's own factor ({expected})"
        );
        published
    };

    // A runner nobody seeded (the brush editor preview, a bare test) sits at
    // the reference, so the factor is an identity there.
    assert!((eval_at(None) - 1.0).abs() < 1e-6);
    assert!((eval_at(Some(REFERENCE_DPI)) - 1.0).abs() < 1e-6);
    assert!((eval_at(Some(REFERENCE_DPI * 2.0)) - 2.0).abs() < 1e-6);
    assert!((eval_at(Some(REFERENCE_DPI / 4.0)) - 0.25).abs() < 1e-6);
}

/// The shader already carries the factor for its own boundary work, so
/// `dpi_scale` reads that field rather than packing a parallel uniform: the
/// same fact in two places is the anti-pattern the document authority
/// principle names. It also means a compiled brush stays document-agnostic
/// and never needs recompiling when the DPI changes.
#[test]
fn dpi_scale_reads_the_intrinsic_uniform_not_its_own() {
    let (graph, node_id) = graph_with_dpi_scaled_grain();
    // Through `compile_graph`, the entry point the engine itself uses.
    let compiled = darkly::brush::compile_graph(&graph)
        .expect("graph compiles")
        .compiled_brush()
        .expect("the graph terminates in `paint`, so it compiles to WGSL");

    // `CompileWgslCtx::uniform_field_name` would have named a node
    // contribution `n{node_id}_{port}`.
    let field = format!("n{}_dpi_scale", node_id.0);
    assert!(
        !compiled.uniform_layout.iter().any(|f| f.name == field),
        "dpi_scale must not pack its own uniform field, got {:?}",
        compiled
            .uniform_layout
            .iter()
            .map(|f| &f.name)
            .collect::<Vec<_>>()
    );
    assert!(
        compiled.stroke_wgsl.contains("u.intrinsic.dpi_factor"),
        "the shader must read the factor from the intrinsic uniforms"
    );
}

/// Spacing is the other half of the dab boundary: it is a fraction of the
/// dab diameter, so at twice the reference DPI the diameter doubles, the
/// interval between dabs doubles with it, and a stroke of fixed canvas
/// length places half as many dabs. That is what keeps a brush's *character*
/// (how densely its marks overlap) the same on any document, rather than
/// only its footprint. Fails before the conversion, where the dab count is
/// DPI-invariant.
#[test]
fn spacing_scales_with_the_document_dpi() {
    let dabs_at = |dpi: f32| -> u64 {
        let (w, h) = (256u32, 256u32);
        let mut engine = test_engine(w, h);
        let layer = engine.add_raster_layer(None);
        engine.rescale_image(w, h, Some(dpi));

        engine.begin_stroke(layer).unwrap();
        // A straight stroke of fixed canvas length, fed in small steps so the
        // dab count is set by the spacing rather than by the event rate.
        for i in 0..=32 {
            engine.stroke_to(StrokeOp::BrushStroke {
                x: 8.0 + i as f32 * 7.0,
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
        engine.test_stroke_total_dabs()
    };

    let reference = dabs_at(REFERENCE_DPI);
    let doubled = dabs_at(REFERENCE_DPI * 2.0);
    assert!(
        reference > 8,
        "the reference stroke placed too few dabs to measure ({reference})"
    );
    let ratio = doubled as f32 / reference as f32;
    assert!(
        (ratio - 0.5).abs() < 0.1,
        "doubling the DPI must double the dab interval, halving the count over \
         a fixed canvas length: {reference} dabs at the reference vs {doubled} at \
         twice it (ratio {ratio})"
    );
}

/// The purpose, end to end, with the **stock brush and no wiring at all**: a
/// document at twice the reference DPI is half the physical size per pixel,
/// so the same brush must paint a mark twice as wide in canvas pixels to
/// stay the same size on the artwork. Fails before the boundary conversion
/// exists, where the dab is a fixed canvas-pixel count on every document.
#[test]
fn a_stroke_scales_with_the_document_dpi() {
    let painted_width_at = |dpi: f32| -> u32 {
        let (w, h) = (256u32, 256u32);
        let mut engine = test_engine(w, h);
        let layer = engine.add_raster_layer(None);
        // A DPI-only edit: same dims, so nothing is resampled.
        engine.rescale_image(w, h, Some(dpi));

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

    let reference = painted_width_at(REFERENCE_DPI);
    let doubled = painted_width_at(REFERENCE_DPI * 2.0);
    assert!(
        reference > 8,
        "the reference dab is too small to measure ({reference} px)"
    );
    let ratio = doubled as f32 / reference as f32;
    assert!(
        (ratio - 2.0).abs() < 0.1,
        "doubling the document's DPI must double the stock brush's canvas-pixel mark: \
         {reference} px at the reference vs {doubled} px at twice it (ratio {ratio})"
    );
}
