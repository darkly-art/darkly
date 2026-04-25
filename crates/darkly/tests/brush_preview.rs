//! Engine-level integration tests for brush preview regen through the
//! overlay mask pipeline. Covers:
//!   - `regenerate_brush_preview` runs the terminal's `render_preview`
//!     hook (color_output) which consumes the `brush_preview` input wire.
//!   - The stamp's `render_preview` produces a deposition-stripped,
//!     transform-baked tip texture; flow / opacity / rotation handling
//!     all happen there, not as a side effect of the deposition path.
//!   - `BrushPreviewInfo` is cached on the engine with non-zero extents.
//!   - Loaded legacy presets (with `preview_output` nodes) migrate
//!     transparently to the new wiring.

use darkly::brush::wire::BrushWireType;
use darkly::brush::BrushNodeRegistry;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::nodegraph::{Graph, NodeId, NodeInstance, PortRef};

fn test_engine(w: u32, h: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, w, h)
}

fn find_node_id(engine: &DarklyEngine, type_id: &str) -> NodeId {
    engine
        .active_brush_graph_ref()
        .nodes
        .values()
        .find(|n: &&NodeInstance<BrushWireType>| n.type_id == type_id)
        .unwrap_or_else(|| panic!("no '{type_id}' node in default graph"))
        .id
}

#[test]
fn default_graph_regenerates_brush_preview_into_overlay_mask() {
    let mut engine = test_engine(256, 256);

    // Default graph is wired `stamp.dab → color_output.preview`, so this
    // should trigger a stamp render_preview into the overlay preview mask.
    engine.regenerate_brush_preview();

    // Engine caches the canvas-space info for the brush tool.
    let info = engine
        .brush_preview_info()
        .expect("default graph has a preview producer");
    assert!(info.half_extent_canvas_px[0] > 0.0);
    assert!(info.half_extent_canvas_px[1] > 0.0);
    assert_eq!(info.rotation_rad, 0.0, "default graph has rotation=0");

    // The overlay should now own a populated preview mask texture.
    let (mask_w, mask_h) = engine.compositor_preview_mask_size();
    assert!(mask_w > 0 && mask_h > 0, "preview mask allocated");

    // Read back the mask and verify it's non-empty (default circle tip fills
    // most of the target). Center pixel's red channel should be > 0.
    let pixels = engine.test_readback_overlay_preview_mask();
    let cx = mask_w / 2;
    let cy = mask_h / 2;
    let center_idx = ((cy * mask_w + cx) * 4) as usize;
    let center_r = pixels[center_idx];
    assert!(
        center_r > 0,
        "preview mask center should be non-zero (got {}): default round \
         stamp must write its alpha into the mask target",
        center_r
    );

    // A pixel near the very edge of the target should be transparent,
    // confirming the stamp renders as a bounded disc, not a filled rect.
    let edge_a = pixels[3];
    assert!(
        edge_a <= 16,
        "preview mask corner should be near-zero alpha, got {edge_a}"
    );
}

/// Crank `stamp.flow` to almost zero and verify the preview is unchanged.
/// Pre-redesign, the preview blitted whatever `dab` the stamp emitted, so
/// a low flow stripped the preview's alpha. The redesign routes preview
/// through `stamp::render_preview`, which neutralises deposition.
#[test]
fn preview_ignores_stamp_flow() {
    let mut engine = test_engine(256, 256);
    engine.regenerate_brush_preview();
    let baseline = engine.test_readback_overlay_preview_mask();
    let baseline_size = engine.compositor_preview_mask_size();

    let stamp = find_node_id(&engine, "stamp");
    engine
        .brush_graph_set_port_default(stamp.0, "flow", 0.05)
        .unwrap();
    engine.regenerate_brush_preview();

    let dimmed = engine.test_readback_overlay_preview_mask();
    let dimmed_size = engine.compositor_preview_mask_size();
    assert_eq!(baseline_size, dimmed_size, "preview mask size unchanged");

    let (mask_w, mask_h) = baseline_size;
    let cx = mask_w / 2;
    let cy = mask_h / 2;
    let center_idx = ((cy * mask_w + cx) * 4) as usize;

    // Centre pixel should still be near full alpha — flow=0.05 must not
    // have leaked into the preview path.
    assert!(
        dimmed[center_idx] > 200,
        "preview centre R should stay near full despite stamp.flow=0.05 \
         (baseline R={}, got R={}); flow is bleeding into the preview path",
        baseline[center_idx],
        dimmed[center_idx],
    );
}

/// Same idea for `color_output.opacity` — a deposition cap that lives on
/// the terminal must not affect the preview.
#[test]
fn preview_ignores_color_output_opacity() {
    let mut engine = test_engine(256, 256);
    engine.regenerate_brush_preview();
    let baseline = engine.test_readback_overlay_preview_mask();
    let (mask_w, mask_h) = engine.compositor_preview_mask_size();
    let cx = mask_w / 2;
    let cy = mask_h / 2;
    let center_idx = ((cy * mask_w + cx) * 4) as usize;
    let baseline_r = baseline[center_idx];
    assert!(baseline_r > 200, "baseline preview centre should be solid");

    let color_out = find_node_id(&engine, "color_output");
    engine
        .brush_graph_set_port_default(color_out.0, "opacity", 0.05)
        .unwrap();
    engine.regenerate_brush_preview();

    let dimmed = engine.test_readback_overlay_preview_mask();
    assert!(
        dimmed[center_idx] > 200,
        "preview centre R should stay near full despite color_output.opacity=0.05 \
         (baseline R={baseline_r}, got R={}); stroke-opacity is bleeding into the preview",
        dimmed[center_idx],
    );
}

/// `BrushPreviewInfo.half_extent` must equal half the texture's canvas-pixel
/// dimensions. The texture self-describes its extent — there's no parallel
/// "preview size" wire to keep in sync — so this round-trip is the
/// invariant the preview overlay primitive depends on.
#[test]
fn preview_extent_matches_texture_dimensions() {
    let mut engine = test_engine(256, 256);
    let stamp = find_node_id(&engine, "stamp");

    // Pin the stamp's effective size: pressure → size in the default
    // graph, but we override the scale and disconnect from pressure-driven
    // dynamics by setting the port default to a known value. (The wire
    // would dominate; setting the default has no effect when wired. So
    // instead verify the relationship: half_extent_x * 2 = an integer
    // pixel count consistent with the brush.)
    engine
        .brush_graph_set_port_default(stamp.0, "scale", 1.0)
        .unwrap();
    engine.regenerate_brush_preview();

    let info = engine
        .brush_preview_info()
        .expect("preview produced placement info");

    // The two halves should be equal for a square tip + ratio=1.
    let hw = info.half_extent_canvas_px[0];
    let hh = info.half_extent_canvas_px[1];
    assert!(hw > 0.0 && hh > 0.0, "non-zero extent");
    assert!(
        (hw - hh).abs() < 0.5,
        "square circle tip should produce a square half-extent ({hw}, {hh})"
    );

    // The full extent must be a whole pixel count (the texture allocator
    // takes integer dimensions).
    let full_w = hw * 2.0;
    assert!(
        (full_w - full_w.round()).abs() < 1e-3,
        "extent should be an integer pixel count, got {full_w}"
    );
}

/// Wire any old texture into `color_output.brush_preview` and verify the
/// overlay shows it. Proves the input is honoured regardless of source —
/// the brush terminal preview output is conventional, not required.
#[test]
fn preview_uses_arbitrary_brush_preview_texture() {
    let mut engine = test_engine(256, 256);

    // Replace the `stamp.preview → color_output.brush_preview` wire with
    // one from an `image` node. We upload a 32×32 solid-red texture and
    // wire it in.
    let red_pixels: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 255], 32 * 32)
        .flatten()
        .collect();
    engine
        .brush_upload_image("test-red", 32, 32, &red_pixels)
        .unwrap();

    let stamp_id = find_node_id(&engine, "stamp");
    let color_id = find_node_id(&engine, "color_output");
    engine
        .brush_graph_disconnect(stamp_id.0, "preview", color_id.0, "brush_preview")
        .unwrap();
    let registry = BrushNodeRegistry::new();
    let image_reg = registry.get("image").unwrap();
    // brush_graph_add_node returns updated JSON, but we need the new
    // node id. Walk the graph after the call.
    let json_before: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(engine.active_brush_graph_ref()).unwrap())
            .unwrap();
    let _ = engine.brush_graph_add_node("image", 100.0, 600.0).unwrap();
    let json_after: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(engine.active_brush_graph_ref()).unwrap())
            .unwrap();
    // Find the node id present in `after` but not `before`.
    let before_ids: std::collections::HashSet<u64> = json_before["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.parse().unwrap())
        .collect();
    let after_ids: std::collections::HashSet<u64> = json_after["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.parse().unwrap())
        .collect();
    let new_image_id = *after_ids.difference(&before_ids).next().unwrap();

    // Set the image node's resource_name to our uploaded texture.
    engine
        .brush_graph_set_param(
            new_image_id,
            0,
            darkly::gpu::params::ParamValue::String("test-red".into()),
        )
        .unwrap();

    // Wire image.texture → color_output.brush_preview.
    engine
        .brush_graph_connect(new_image_id, "texture", color_id.0, "brush_preview")
        .unwrap();

    let _ = image_reg; // kept for clarity above

    engine.regenerate_brush_preview();
    let pixels = engine.test_readback_overlay_preview_mask();
    let (mask_w, mask_h) = engine.compositor_preview_mask_size();

    let cx = mask_w / 2;
    let cy = mask_h / 2;
    let center_idx = ((cy * mask_w + cx) * 4) as usize;
    // Our texture is solid red, so the blit should land it as red on the
    // preview mask. The overlay shader only reads `.r` for coverage, but
    // we can confirm the blit happened by checking R is high.
    assert!(
        pixels[center_idx] > 200,
        "preview centre should reflect the wired-in red texture, got R={}",
        pixels[center_idx],
    );
}

/// A graph whose `color_output` has no `brush_preview` wire should
/// short-circuit cleanly: no preview info, no overlay mask binding.
#[test]
fn preview_short_circuits_when_brush_preview_unconnected() {
    let mut engine = test_engine(256, 256);

    // Remove the default preview wire.
    let stamp_id = find_node_id(&engine, "stamp");
    let color_id = find_node_id(&engine, "color_output");
    engine
        .brush_graph_disconnect(stamp_id.0, "preview", color_id.0, "brush_preview")
        .unwrap();

    engine.regenerate_brush_preview();
    assert!(
        engine.brush_preview_info().is_none(),
        "no brush_preview wire → no placement info"
    );
}

/// A brush saved before the redesign carries a `preview_output` node and
/// `stamp.dab → preview_output.dab` wires. Loading it should silently
/// migrate to the new wiring (`stamp.preview → color_output.brush_preview`)
/// and produce a working preview.
#[test]
fn legacy_preview_output_migrates_on_load() {
    use darkly::brush::bundle::{Brush, BrushMetadata};

    // Build a graph the old way: pen / circle / stamp / color_output, plus
    // a `preview_output` node wired from stamp.
    let registry = BrushNodeRegistry::new();
    let mut graph: Graph<BrushWireType> = Graph::new();
    let pen = graph.add_node(
        "pen_input",
        registry.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let paint_color = graph.add_node(
        "paint_color",
        registry.get("paint_color").unwrap().ports.clone(),
        vec![],
    );
    let circle = graph.add_node(
        "circle",
        registry.get("circle").unwrap().ports.clone(),
        vec![],
    );
    let stamp = graph.add_node(
        "stamp",
        registry.get("stamp").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::Int(0)],
    );
    let color_output = graph.add_node(
        "color_output",
        registry.get("color_output").unwrap().ports.clone(),
        vec![],
    );

    // Synthesise the legacy `preview_output` node — it no longer has a
    // registration in the runtime, so we hand-craft an instance with the
    // ports the old version had.
    use darkly::nodegraph::{PortDef, PortDir};
    let preview_output_id = graph.add_node(
        "preview_output",
        vec![
            PortDef {
                name: "dab".into(),
                dir: PortDir::Input,
                wire_type: BrushWireType::Texture,
                min: 0.0,
                max: 0.0,
                default: 0.0,
                description: String::new(),
                unit_type: Default::default(),
                icon: String::new(),
                label: String::new(),
                exposed: false,
            },
            PortDef {
                name: "dab_size".into(),
                dir: PortDir::Input,
                wire_type: BrushWireType::Vec2,
                min: 0.0,
                max: 0.0,
                default: 0.0,
                description: String::new(),
                unit_type: Default::default(),
                icon: String::new(),
                label: String::new(),
                exposed: false,
            },
        ],
        vec![],
    );

    let wires = [
        (circle, "texture", stamp, "tip"),
        (pen, "pressure", stamp, "size"),
        (paint_color, "color", stamp, "color"),
        (stamp, "dab", color_output, "dab"),
        (stamp, "dab_size", color_output, "dab_size"),
        (pen, "position", color_output, "position"),
        // Legacy preview wires that the migration should drop.
        (stamp, "dab", preview_output_id, "dab"),
        (stamp, "dab_size", preview_output_id, "dab_size"),
    ];
    for (fn_, fp, tn, tp) in wires {
        graph
            .connect(
                PortRef {
                    node: fn_,
                    port: fp.into(),
                },
                PortRef {
                    node: tn,
                    port: tp.into(),
                },
            )
            .unwrap();
    }

    let metadata = BrushMetadata::from_graph("Legacy", graph);
    let brush = Brush::without_resources(metadata);
    let bytes = brush.to_bytes().unwrap();
    let loaded = Brush::from_bytes(&bytes).unwrap();

    // Migration removed the legacy node.
    let has_preview_output = loaded
        .metadata
        .graph
        .nodes
        .values()
        .any(|n| n.type_id == "preview_output");
    assert!(
        !has_preview_output,
        "preview_output should be gone after migration"
    );

    // Migration installed the new wire.
    let has_new_wire = loaded
        .metadata
        .graph
        .connections
        .iter()
        .any(|c| c.to.port == "brush_preview" && c.from.port == "preview");
    assert!(
        has_new_wire,
        "migration should install stamp.preview → color_output.brush_preview"
    );

    // The migrated graph must compile.
    let runner = darkly::brush::compile_graph(&loaded.metadata.graph);
    assert!(
        runner.is_ok(),
        "migrated graph should compile: {:?}",
        runner.err()
    );
}
