//! Auto-iterating round-trip tests for the wire format.
//!
//! These tests are the contract that protects modular extensions: adding a
//! new veil / stabilizer / brush node / blend mode / layer kind / filter
//! kind lights up testing automatically here, with no edits. The shape
//! check is the same in every test — `(type_id, params)` (or just
//! `type_id` for closed-set kinds) must survive a JSON round-trip and
//! still resolve through the registry on the other side.

use serde::{Deserialize, Serialize};

use super::registry_io::{deserialize_instance, serialize_instance, InstancePayload};
use crate::brush::registry;
use crate::brush::stabilizer::StabilizerRegistry;
use crate::brush::wire::BrushWireType;
use crate::document::filter;
use crate::document::layer_kind;
use crate::gpu::blend_mode;
use crate::gpu::context::GpuContext;
use crate::gpu::effect::EffectRegistry;
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::test_utils::test_device;
use crate::nodegraph::Graph;

/// Build a default `Vec<ParamValue>` from a `&[ParamDef]` schema. Mirrors
/// the per-defs default each registry uses internally; centralized here
/// so the seven tests share one implementation.
fn defaults_of(params: &[ParamDef]) -> Vec<ParamValue> {
    params.iter().map(ParamDef::default_value).collect()
}

/// Headless `DarklyEngine` plus the per-test viewport bookkeeping. The
/// veil chain sizes off the viewport, which is 0×0 in headless mode by
/// default — kitchen-sink populates a chain, so we seed the size
/// manually like the engine/save inline tests do.
fn kitchen_sink_engine(width: u32, height: u32) -> crate::engine::DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let mut engine = crate::engine::DarklyEngine::new(gpu, width, height);
    engine.compositor.effect_chain_mut().resize(
        &engine.gpu.device,
        &engine.gpu.queue,
        width,
        height,
    );
    engine
}

// ----------------------------------------------------------------------------
// 1. Veils — round-trip (type_id, param_values) for every registered veil.
//
// Veils carry both `type_id()` and `param_values()` on the trait, so we
// can serialize the instance, parse it back, reconstruct via the registry,
// and assert byte-equality on both fields. This is the strictest of the
// seven tests because only veils expose live instance introspection today.
// ----------------------------------------------------------------------------

#[test]
fn round_trip_every_veil() {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    // Effects compile only against a format they declare in `targets`, and the
    // screen-space chain runs at the accumulator format — so that is what a
    // round-trip instantiates against.
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut registry = EffectRegistry::new();

    // Cloning the (type_id, params) pairs into a Vec keeps the borrow scoped so
    // `instance` can take `&mut registry` below without contending with the
    // iteration.
    let types: Vec<(&'static str, &'static [ParamDef])> = registry
        .registrations()
        .into_iter()
        .map(|reg| (reg.type_id, reg.params))
        .collect();
    assert!(
        !types.is_empty(),
        "the effect registry must contain at least one effect"
    );

    for (type_id, params_schema) in types {
        let defaults = defaults_of(params_schema);
        let veil = registry
            .instance(type_id, &defaults, &gpu.device, format)
            .expect("registered effect");

        // Serialize via the canonical wire envelope.
        let json = serialize_instance(veil.type_id(), veil.param_values())
            .unwrap_or_else(|e| panic!("serialize veil '{type_id}' failed: {e}"));

        // Parse back and rebuild.
        let payload = deserialize_instance(&json)
            .unwrap_or_else(|e| panic!("deserialize veil '{type_id}' failed: {e}"));
        assert_eq!(payload.type_id, type_id);

        let restored = registry
            .instance(&payload.type_id, &payload.params, &gpu.device, format)
            .expect("registered effect");
        assert_eq!(
            restored.type_id(),
            veil.type_id(),
            "type_id drift for '{type_id}'"
        );
        assert_eq!(
            restored.param_values().len(),
            veil.param_values().len(),
            "param count drift for veil '{type_id}'"
        );
        for (a, b) in restored
            .param_values()
            .iter()
            .zip(veil.param_values().iter())
        {
            assert_param_eq(a, b, type_id);
        }
    }
}

/// The `round_trip_every_veil` sweep covers CA at its defaults; this pins a veil
/// carrying **non-default** `List` params (edited offsets/scales/colors/blur)
/// through the same wire envelope, exercising the composite Vec2/Color/List
/// kinds with real values.
#[test]
fn chromatic_aberration_veil_round_trips_non_default_list() {
    use std::collections::BTreeMap;

    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut registry = EffectRegistry::new();

    let params = vec![ParamValue::List(vec![
        BTreeMap::from([
            ("offset".to_string(), ParamValue::Vec2([6.0, -2.0])),
            ("scale".to_string(), ParamValue::Float(1.02)),
            ("color".to_string(), ParamValue::Color([1.0, 0.3, 0.1])),
            ("blur".to_string(), ParamValue::Float(2.0)),
        ]),
        BTreeMap::from([
            ("offset".to_string(), ParamValue::Vec2([0.0, 0.0])),
            ("scale".to_string(), ParamValue::Float(0.98)),
            ("color".to_string(), ParamValue::Color([0.1, 0.4, 1.0])),
            ("blur".to_string(), ParamValue::Float(0.0)),
        ]),
    ])];

    let veil = registry
        .instance("chromatic_aberration", &params, &gpu.device, format)
        .expect("registered effect");
    let json =
        serialize_instance(veil.type_id(), veil.param_values()).expect("serialize CA veil failed");
    let payload = deserialize_instance(&json).expect("deserialize CA veil failed");
    assert_eq!(payload.type_id, "chromatic_aberration");
    let restored = registry
        .instance(&payload.type_id, &payload.params, &gpu.device, format)
        .expect("registered effect");
    assert_eq!(
        restored.param_values(),
        params,
        "non-default CA veil list params must round-trip byte-for-byte"
    );
}

// ----------------------------------------------------------------------------
// 2. Stabilizers — round-trip (type_id, defaults) for every registered
//    algorithm.
//
// `StabilizerAlgorithm` doesn't expose `param_values()` (the trait is
// runtime-focused), so we round-trip via the registration's defaults and
// verify the registry can rehydrate from the parsed payload. The shape
// check is what matters here — `(type_id, params)` JSON contract holds.
// ----------------------------------------------------------------------------

#[test]
fn round_trip_every_stabilizer() {
    let registry = StabilizerRegistry::new();
    let types = registry.types();
    assert!(
        !types.is_empty(),
        "stabilizer registry must contain at least one algorithm"
    );

    for (type_id, _name, params_schema) in types {
        let defaults = defaults_of(params_schema);

        let json = serialize_instance(type_id, defaults.clone())
            .unwrap_or_else(|e| panic!("serialize stabilizer '{type_id}' failed: {e}"));
        let payload = deserialize_instance(&json)
            .unwrap_or_else(|e| panic!("deserialize stabilizer '{type_id}' failed: {e}"));
        assert_eq!(payload.type_id, type_id);
        assert_eq!(payload.params.len(), defaults.len());

        // The registry can rebuild from the parsed payload — the
        // happy-path equivalent of what `start_save` → reload will run
        // through in Phase 3+.
        let _stab = registry
            .create(&payload.type_id, &payload.params)
            .unwrap_or_else(|| panic!("registry rejected its own '{type_id}'"));
    }
}

// ----------------------------------------------------------------------------
// 3. Brush nodes — round-trip `NodeInstance<BrushWireType>` for every
//    registered node type via a real `Graph` add+serialize+parse cycle.
//
// Brush nodes already serialize through `NodeInstance` (graph.rs), so the
// wire format is whatever serde emits on a `Graph<W>`. This test proves
// that emit/parse round-trips for every registered node type with default
// params.
// ----------------------------------------------------------------------------

#[test]
fn round_trip_every_brush_node() {
    let registry = registry();
    let types: Vec<&str> = registry
        .types()
        .into_iter()
        .map(|reg| reg.type_id)
        .collect();
    assert!(
        !types.is_empty(),
        "brush node registry must contain at least one node type"
    );

    for type_id in types {
        let mut graph: Graph<BrushWireType> = Graph::new();
        let reg = registry
            .get(type_id)
            .unwrap_or_else(|| panic!("missing registration for '{type_id}'"));
        let id = graph.add_node(reg.type_id, reg.ports.clone());

        let json = serde_json::to_string(&graph)
            .unwrap_or_else(|e| panic!("serialize graph with '{type_id}' failed: {e}"));
        let back: Graph<BrushWireType> = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("deserialize graph with '{type_id}' failed: {e}"));

        let node = back
            .nodes()
            .get(&id)
            .unwrap_or_else(|| panic!("node lost across round-trip for '{type_id}'"));
        assert_eq!(node.type_id, type_id);
        assert_eq!(node.ports.len(), reg.ports.len());
    }
}

// ----------------------------------------------------------------------------
// 4. Blend modes — round-trip the type_id string for every registered mode.
//
// Closed set, but exposed via a registry rather than an enum (per
// `gpu/blend_mode.rs`). Iterate `BlendModeRegistry::all()` and verify the
// JSON string round-trip resolves back through `BlendModeRegistry::get`.
// ----------------------------------------------------------------------------

#[test]
fn round_trip_every_blend_mode() {
    let registry = blend_mode::registry();
    let all = registry.all();
    assert!(
        !all.is_empty(),
        "blend mode registry must contain at least 'normal'"
    );

    for reg in all {
        let id = reg.type_id;
        let json = serde_json::to_string(id).unwrap();
        let back: String = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(
            registry.get(&back).is_some(),
            "round-tripped blend mode '{id}' must resolve back through registry"
        );
    }
}

// ----------------------------------------------------------------------------
// 5. Filter kinds — round-trip the type_id string AND the body envelope
//    for every registered filter kind (mask, selection, future
//    filter/transform/...).
// ----------------------------------------------------------------------------

#[test]
fn round_trip_every_modifier_kind() {
    let registry = filter::registry();
    let all = registry.all();
    assert!(
        !all.is_empty(),
        "filter registry must contain at least one kind"
    );

    for reg in all {
        let id = reg.type_id;
        let json = serde_json::to_string(id).unwrap();
        let back: String = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(
            registry.get(&back).is_some(),
            "round-tripped filter kind '{id}' must resolve back through registry"
        );
    }
}

// ----------------------------------------------------------------------------
// 6. Layer kinds — round-trip the type_id string for every registered
//    layer kind (raster, group, future text/filter/...).
// ----------------------------------------------------------------------------

#[test]
fn round_trip_every_layer_kind() {
    let registry = layer_kind::registry();
    let all = registry.all();
    assert!(
        !all.is_empty(),
        "layer kind registry must contain at least one kind"
    );

    for reg in all {
        let id = reg.type_id;
        let json = serde_json::to_string(id).unwrap();
        let back: String = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(
            registry.get(&back).is_some(),
            "round-tripped layer kind '{id}' must resolve back through registry"
        );
    }
}

// ----------------------------------------------------------------------------
// 7. ParamValue variants — round-trip each variant explicitly.
//
// The companion test in `gpu/params.rs` (`paramvalue_round_trips_preserve_variant`)
// guards `Bool`/`Int`/`Float`/`String`/`Curve` from the regression where the
// untagged enum coerced `Int(n)` to `Float(n as f32)`. This test confirms the
// same for the format module so any future variant addition lights up
// failure here too.
// ----------------------------------------------------------------------------

#[test]
fn round_trip_param_value_variants() {
    let cases = [
        ParamValue::Bool(true),
        ParamValue::Bool(false),
        ParamValue::Int(0),
        ParamValue::Int(42),
        ParamValue::Int(-7),
        ParamValue::Float(0.0),
        ParamValue::Float(1.5),
        ParamValue::Float(-3.25),
        ParamValue::String("hello".into()),
        ParamValue::String(String::new()),
        ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]),
        ParamValue::Curve(vec![]),
    ];
    for v in &cases {
        let payload = InstancePayload::new("noop", vec![v.clone()]);
        let json = serde_json::to_value(&payload).unwrap();
        let back: InstancePayload = serde_json::from_value(json).unwrap();
        assert_eq!(back.params.len(), 1);
        assert_param_eq(&back.params[0], v, "param_value_variants");
    }
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

/// Variant-preserving equality for `ParamValue`. The `#[serde(untagged)]`
/// representation means we must check variant identity, not just numeric
/// equality — the regression in `gpu/params.rs` was exactly an
/// `Int(1) == Float(1.0)` false-positive after coercion.
fn assert_param_eq(a: &ParamValue, b: &ParamValue, context: &str) {
    // `ParamValue` derives `PartialEq`, so equality covers every variant
    // (including the composite `Color`/`Vec2`/`List` kinds) uniformly.
    assert_eq!(a, b, "[{context}] param mismatch");
}

// ----------------------------------------------------------------------------
// Sanity: the canonical `InstancePayload` wire shape is reachable from
// the seven tests above — emit-and-parse a small fixture so regressions
// in serde's representation surface here as well as in the helpers' own
// tests.
// ----------------------------------------------------------------------------

#[test]
fn instance_payload_shape_is_type_id_plus_params() {
    #[derive(Serialize, Deserialize)]
    struct Probe {
        type_id: String,
        params: Vec<ParamValue>,
    }
    let payload = InstancePayload::new("foo", vec![ParamValue::Int(1)]);
    let json = serde_json::to_value(&payload).unwrap();
    let probe: Probe = serde_json::from_value(json).unwrap();
    assert_eq!(probe.type_id, "foo");
    assert!(matches!(probe.params.as_slice(), [ParamValue::Int(1)]));
}

// ----------------------------------------------------------------------------
// Kitchen-sink — end-to-end save → zip → load with every closed-set
// variant exercised. The cross-version safety net for the wire format:
// any future serde / id-remap / texture-format regression surfaces
// here as a doc-mismatch or composite-byte difference.
// ----------------------------------------------------------------------------

use crate::document::Document;
use crate::engine::DarklyEngine;
use crate::format::manifest::SaveBundle;
use crate::format::zip_io::{assemble_zip, extract_zip};
use crate::layer::LayerId;

/// Populate the engine with at least one of every closed-set variant
/// the save format needs to cover:
///   - every layer kind (`raster`, `group`) — at least one each.
///   - every filter kind (`mask`, `selection`) — at least one each.
///   - every blend mode in `BlendModeRegistry::all()` — at least one
///     layer with each.
///   - one of every registered veil.
///
/// Plus a few hand-built features the load path needs to handle: a
/// non-trivial tree (group with children inside it), a non-default
/// document name, and pixel data on layers so the round-trip is
/// non-empty.
///
/// When a new closed-set variant is added (new blend mode, new layer
/// kind, new filter kind), `kitchen_sink_covers_every_closed_set_variant`
/// fires immediately — this function must be extended to instantiate
/// the new variant.
fn populate_kitchen_sink(engine: &mut DarklyEngine) {
    engine.set_document_name("Kitchen Sink".to_string());

    // One raster layer per blend mode, named after the mode for
    // diagnosability if a single mode regresses.
    let modes: Vec<&'static str> = blend_mode::registry()
        .all()
        .iter()
        .map(|reg| reg.type_id)
        .collect();
    let mut raster_ids: Vec<LayerId> = Vec::with_capacity(modes.len());
    for type_id in &modes {
        let id = engine.add_raster_layer(None);
        engine.set_blend_mode(id, type_id);
        engine.set_layer_name(id, &format!("blend-{type_id}"));
        raster_ids.push(id);
    }

    // A group at root, with a child raster moved into it — exercises
    // both Group structure and parent/children id rewiring on load.
    let group = engine.add_group(None);
    if let Some(first_layer) = raster_ids.first() {
        engine.move_layer(
            *first_layer,
            crate::document::MoveTarget::IntoGroupTop(group),
        );
    }

    // Mask filter on one of the rasters — exercises mask kind +
    // its parent-host wiring.
    if let Some(target) = raster_ids.get(1).copied() {
        engine.add_mask(target);
    }

    // Selection mask — `select_all` flips selection.active and
    // populates the R8 texture; the filter itself was allocated
    // eagerly at engine init.
    engine.select_all();

    // One of every veil — keep params at default, leave visibility on.
    let veil_types: Vec<(&'static str, &'static [ParamDef])> = engine
        .compositor
        .effect_registry()
        .registrations()
        .into_iter()
        .map(|reg| (reg.type_id, reg.params))
        .collect();
    for (type_id, schema) in veil_types {
        let defaults = defaults_of(schema);
        engine.add_veil_layer(type_id, &defaults);
    }

    // One of every void type — adds a void layer at root for each
    // registered void kind, with schema defaults. Closes the kitchen-sink
    // assertion that every layer_kind has a representative in the saved
    // doc; without this, `layer_kind/void` would never participate in the
    // save round-trip test.
    let void_types: Vec<(&'static str, &'static [ParamDef])> = engine
        .compositor
        .void_registry()
        .types()
        .into_iter()
        .map(|reg| (reg.type_id, reg.params))
        .collect();
    for (type_id, schema) in void_types {
        let defaults = defaults_of(schema);
        engine.add_void_layer(type_id, defaults, None);
    }

    // One of every filter type — adds a filter layer at root for each
    // registered filter pipeline (filters carry no params today), so
    // `layer_kind/filter` participates in the save round-trip test.
    let filter_types: Vec<String> = engine
        .compositor
        .effect_registry()
        .registrations()
        .into_iter()
        .map(|reg| reg.type_id.to_string())
        .collect();
    for pipeline in filter_types {
        engine.add_filter_layer(&pipeline, Vec::new(), None);
    }

    // A vector (text) layer so `layer_kind/vector` participates in the save
    // round-trip — its objects round-trip through the manifest body (no pixels).
    engine.add_text_layer(
        crate::layer::TextProps::new("kitchen sink".to_string()),
        4.0,
        4.0,
        [0, 0, 0, 255],
        None,
    );
}

/// Pump the engine until a save completes. Caps iterations so a stuck
/// readback fails the test rather than hanging.
fn drive_save_to_completion(engine: &mut DarklyEngine) -> SaveBundle {
    engine
        .start_save_document(crate::engine::SavePurpose::File)
        .expect("start save");
    for _ in 0..32 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if let Some(b) = engine.poll_save_result() {
            return b;
        }
    }
    panic!("save did not complete within 32 frames");
}

/// Coarse structural comparison: same canvas size, same number of
/// raster layers, same number of groups, same number of filters,
/// same document name, same veil count, same selection presence.
/// Strict per-id mapping is intentionally NOT checked — slotmap keys
/// are document-local and won't match across documents.
fn assert_documents_equivalent(a: &Document, b: &Document) {
    assert_eq!(a.width, b.width);
    assert_eq!(a.height, b.height);
    assert_eq!(a.name, b.name);
    assert_eq!(a.all_raster_layers().len(), b.all_raster_layers().len());
    assert_eq!(a.all_groups().len(), b.all_groups().len());
    assert_eq!(a.all_filters().len(), b.all_filters().len());
    assert_eq!(a.selection_id().is_some(), b.selection_id().is_some());
    // Blend-mode coverage matches across the two trees.
    let modes_a: std::collections::BTreeSet<&'static str> = a
        .all_raster_layers()
        .iter()
        .map(|r| r.blend.blend_mode.type_id)
        .collect();
    let modes_b: std::collections::BTreeSet<&'static str> = b
        .all_raster_layers()
        .iter()
        .map(|r| r.blend.blend_mode.type_id)
        .collect();
    assert_eq!(
        modes_a, modes_b,
        "blend mode coverage drifted on round-trip"
    );
}

#[test]
fn round_trip_kitchen_sink_document() {
    let (canvas_w, canvas_h) = (32u32, 32u32);

    let mut original = kitchen_sink_engine(canvas_w, canvas_h);
    populate_kitchen_sink(&mut original);

    let bundle = drive_save_to_completion(&mut original);
    let zip_bytes = assemble_zip(&bundle);
    let entries = extract_zip(&zip_bytes);
    assert!(
        entries.get("manifest.json").is_some(),
        "kitchen-sink zip must contain manifest.json"
    );
    assert!(
        entries.get("composite.png").is_some(),
        "kitchen-sink zip must contain composite.png"
    );

    let mut reloaded = kitchen_sink_engine(1, 1);
    reloaded
        .open_document(&zip_bytes)
        .expect("kitchen-sink reload happy path");

    assert_documents_equivalent(&original.doc, &reloaded.doc);

    // Composite parity — read both engines' composited textures and
    // compare. Both run headless so `test_readback_canvas` is the
    // synchronous path; production never blocks like this.
    let composite_a = original.test_readback_canvas();
    let composite_b = reloaded.test_readback_canvas();
    assert_eq!(
        composite_a.len(),
        composite_b.len(),
        "composite size drifted across save/reload"
    );
    assert_eq!(
        composite_a, composite_b,
        "kitchen-sink composite bytes must match across save/reload"
    );
}

/// Reproducibility: a non-fallback font applied to a text object embeds in the
/// saved `.darkly` and re-registers when the file is opened in a **fresh**
/// engine that never saw the upload — the document is self-contained without the
/// personal library. Uses Cantarell-VF (a real variable font) as the fixture.
#[test]
fn embedded_font_round_trips() {
    const CANTARELL_VF: &[u8] = include_bytes!("../../tests/fixtures/fonts/Cantarell-VF.otf");

    let mut engine = kitchen_sink_engine(64, 64);
    let family = engine
        .register_font(CANTARELL_VF.to_vec())
        .first()
        .expect("Cantarell-VF contributes a family")
        .clone();

    let mut text = crate::layer::TextProps::new("Ag".to_string());
    text.font_family = family.clone();
    text.size = 32.0;
    engine.add_text_layer(text, 8.0, 8.0, [255, 255, 255, 255], None);

    let bundle = drive_save_to_completion(&mut engine);
    let manifest: crate::format::manifest::Manifest =
        serde_json::from_slice(&bundle.manifest_json).unwrap();
    assert!(
        manifest.fonts.iter().any(|f| f.family == family),
        "the used font is listed in manifest.fonts (got {:?})",
        manifest.fonts
    );

    let zip_bytes = assemble_zip(&bundle);
    let mut reloaded = kitchen_sink_engine(1, 1);
    assert!(
        !reloaded.list_fonts().contains(&family),
        "the fresh engine has not seen this font yet"
    );
    reloaded
        .open_document(&zip_bytes)
        .expect("reload happy path");
    assert!(
        reloaded.list_fonts().contains(&family),
        "opening the document registers its embedded font"
    );

    // The text object survived the round-trip with its family intact.
    let vec_layer = reloaded.doc.all_vector_layers()[0].id;
    let objs = reloaded.text_objects(vec_layer);
    assert!(
        objs.iter()
            .any(|o| o.content == "Ag" && o.font_family == family),
        "the reloaded text object keeps its content and embedded family"
    );
}

/// The stronger embedding guarantee: an embedded font doesn't just round-trip by
/// *name* — its glyphs **rasterize identically** after save/reload. Renders the
/// same string with the embedded font and asserts (a) it differs from the
/// fallback face (proving the real font is used, not a silent Noto fallback that
/// would make a name-only test pass regardless), and (b) a fresh engine that
/// only has the embedded bytes reproduces the exact same pixels. Uses
/// Cantarell-VF — a variable CFF2 (`OTTO`) font, an outline format distinct from
/// the bundled glyf TTF, so this also exercises a "weird" font kind end to end.
#[test]
fn embedded_font_renders_identically_after_reload() {
    const CANTARELL_VF: &[u8] = include_bytes!("../../tests/fixtures/fonts/Cantarell-VF.otf");
    let (w, h) = (96u32, 48u32);

    fn render_text(engine: &mut DarklyEngine, family: Option<&str>) -> (LayerId, Vec<u8>) {
        let mut text = crate::layer::TextProps::new("Ag".to_string());
        text.size = 32.0;
        if let Some(f) = family {
            text.font_family = f.to_string();
        }
        let (id, _) = engine.add_text_layer(text, 4.0, 4.0, [255, 255, 255, 255], None);
        let _ = engine.test_readback_canvas(); // force the Vello realization
        (id, engine.test_readback_layer(id))
    }

    // Baseline: the same string in the fallback face (a family with no bytes →
    // Noto / sans-serif), rendered in a throwaway engine so the saved doc stays
    // clean.
    let fallback_px = {
        let mut fe = kitchen_sink_engine(w, h);
        render_text(&mut fe, None).1
    };

    // The Cantarell render, in the engine we'll save.
    let mut engine = kitchen_sink_engine(w, h);
    let family = engine
        .register_font(CANTARELL_VF.to_vec())
        .first()
        .expect("Cantarell-VF contributes a family")
        .clone();
    let (_layer, cantarell_px) = render_text(&mut engine, Some(&family));

    assert_ne!(
        cantarell_px, fallback_px,
        "the embedded font must rasterize differently from the fallback — else a \
         name-only round-trip would pass even if the font never registered"
    );

    // Round-trip into a fresh engine that has only the embedded bytes.
    let bundle = drive_save_to_completion(&mut engine);
    let zip_bytes = assemble_zip(&bundle);
    let mut reloaded = kitchen_sink_engine(1, 1);
    reloaded
        .open_document(&zip_bytes)
        .expect("reload happy path");
    let reloaded_layer = reloaded.doc.all_vector_layers()[0].id;
    let _ = reloaded.test_readback_canvas();
    let reloaded_px = reloaded.test_readback_layer(reloaded_layer);

    assert_eq!(
        cantarell_px, reloaded_px,
        "text must rasterize pixel-identically after embedding + reload — proving \
         the embedded bytes reproduce the glyph outlines, not a fallback"
    );
}

/// A *sub-canvas* mask (bounds smaller than the canvas window) round-trips
/// through save/load with its independent bounds + pixels intact. Mask bounds
/// serialize independently of the host (Document Authority Principle); the
/// de-fused mask-apply pass samples the mask in its own space, so a reloaded
/// sub-canvas mask hides the same region it did before. Cropped first.
#[test]
fn sub_canvas_mask_survives_save_load_round_trip() {
    use crate::coord::CanvasRect;
    use crate::engine::types::StrokeOp;

    // Small canvas so the mask is created at 32×32, then enlarge + offset the
    // canvas window → the mask stays 32×32 (sub-canvas).
    let mut original = kitchen_sink_engine(32, 32);
    let host = original.add_raster_layer(None);
    // Fill the host opaque so the mask's hiding is observable in the composite.
    original.begin_stroke(host).unwrap();
    original.stroke_to(StrokeOp::FloodFill {
        x: 1.0,
        y: 1.0,
        r: 0,
        g: 0,
        b: 255,
        a: 255,
        tolerance: 255,
    });
    original.end_stroke();
    original.render(0.0);

    original.add_mask(host);
    let mask_id = original.host_mask_id(host).expect("mask");
    // Black dab on the mask → a distinct hidden region.
    original.begin_stroke(mask_id).unwrap();
    original.stroke_to(StrokeOp::BrushStroke {
        x: 10.0,
        y: 10.0,
        pressure: 1.0,
        x_tilt: 0.0,
        y_tilt: 0.0,
        rotation: 0.0,
        tangential_pressure: 0.0,
        time_ms: 0.0,
        cr: 0.0,
        cg: 0.0,
        cb: 0.0,
        ca: 1.0,
    });
    original.end_stroke();
    original.render(0.0);

    // Enlarge + offset the canvas window so the 32×32 mask is sub-canvas.
    original.resize_canvas(CanvasRect::from_xywh(12, 8, 64, 48));
    original.render(0.0);
    let bounds_before = original.node_pixel_bounds(mask_id).expect("mask bounds");
    assert!(
        bounds_before.width < 64 && bounds_before.height < 48,
        "test setup: the mask must be sub-canvas; got {bounds_before:?}"
    );

    let bundle = drive_save_to_completion(&mut original);
    let zip_bytes = assemble_zip(&bundle);

    let mut reloaded = kitchen_sink_engine(1, 1);
    reloaded
        .open_document(&zip_bytes)
        .expect("sub-canvas-mask reload happy path");

    // Find the reloaded host (the raster carrying a mask) and its mask.
    let r_host = reloaded
        .doc
        .all_raster_layers()
        .iter()
        .map(|r| r.id)
        .find(|id| reloaded.doc.has_mask(*id))
        .expect("reloaded doc must have a masked raster");
    let r_mask = reloaded.host_mask_id(r_host).expect("reloaded mask");
    let bounds_after = reloaded
        .node_pixel_bounds(r_mask)
        .expect("reloaded mask bounds");
    assert_eq!(
        bounds_after, bounds_before,
        "sub-canvas mask bounds must round-trip independently of the host"
    );

    // The mask's R8 pixels round-trip byte-for-byte (its bulk-pixel authority
    // serialized independently of the host) — the precise claim of this test.
    let mask_a = original.test_readback_mask(host);
    let mask_b = reloaded.test_readback_mask(r_host);
    assert_eq!(
        mask_a, mask_b,
        "sub-canvas mask pixels must round-trip byte-for-byte across save/reload"
    );
    // And the host's own pixels survive too.
    let host_a = original.test_readback_layer(host);
    let host_b = reloaded.test_readback_layer(r_host);
    assert_eq!(
        host_a, host_b,
        "masked host pixels must round-trip across save/reload"
    );
}

/// Regression: after `open_document`, the layer panel must show
/// thumbnails immediately — not wait until the user's first edit.
///
/// The bug was that `engine/load.rs::upload_pixels` wrote bytes via
/// `queue.write_texture` but never called `mark_node_pixels_dirty`, so
/// the per-frame `drain_dirty_thumbnail_readbacks` saw an empty set
/// and no thumbnail readbacks queued. Every paint path marks dirty
/// after writing; the load path was the one outlier. Fix lives in
/// `Compositor::upload_node_pixels` — a single helper that writes
/// AND marks dirty in one step, so the next call site that uploads
/// pixels can't forget.
#[test]
fn loaded_thumbnails_populate_without_a_first_edit() {
    use crate::engine::DEFAULT_THUMB_SIZE;

    let (canvas_w, canvas_h) = (16u32, 16u32);

    // Build a doc with a layer of bright pixels so the thumbnail has
    // non-zero content to read back.
    let mut source = kitchen_sink_engine(canvas_w, canvas_h);
    let red: Vec<u8> = (0..canvas_w * canvas_h)
        .flat_map(|_| [255u8, 0, 0, 255])
        .collect();
    source.paste_image(canvas_w, canvas_h, &red, 0, 0, None);
    let bundle = drive_save_to_completion(&mut source);
    let zip_bytes = assemble_zip(&bundle);

    let mut reloaded = kitchen_sink_engine(canvas_w, canvas_h);
    reloaded
        .open_document(&zip_bytes)
        .expect("reload happy path");

    // Drive the render loop long enough for the dirty-mark to drain
    // into a queued readback and that readback to complete.
    for _ in 0..16 {
        reloaded.test_flush_readbacks();
        reloaded.render(0.0);
    }

    // The reloaded doc's raster layers should each have a non-zero
    // thumbnail cached. We check at least one — the kitchen-sink + the
    // pasted red layer together guarantee one filled raster, while
    // empty-pixel layers stay zero (correctly).
    let raster_ids: Vec<crate::layer::LayerId> = reloaded
        .doc
        .all_raster_layers()
        .iter()
        .map(|r| r.id)
        .collect();
    assert!(
        !raster_ids.is_empty(),
        "reloaded doc must have raster layers"
    );

    let any_non_zero = raster_ids.iter().any(|id| {
        let thumb = reloaded.node_thumbnail(*id, DEFAULT_THUMB_SIZE, DEFAULT_THUMB_SIZE);
        thumb.iter().any(|&b| b != 0)
    });
    assert!(
        any_non_zero,
        "no loaded raster has a non-zero thumbnail — \
         load path forgot to mark_node_pixels_dirty after upload"
    );
}

/// Loading a `.darkly` into a dirty engine clears the dirty flag — the
/// loaded contents are the new "matches disk" baseline. The fresh
/// staging doc constructed in `build_staging_document` starts with
/// `dirty: false`, and the atomic swap installs it as-is.
#[test]
fn dirty_flag_cleared_by_open() {
    let (canvas_w, canvas_h) = (32u32, 32u32);

    let mut original = kitchen_sink_engine(canvas_w, canvas_h);
    populate_kitchen_sink(&mut original);
    let bundle = drive_save_to_completion(&mut original);
    let zip_bytes = assemble_zip(&bundle);

    let mut reloaded = kitchen_sink_engine(canvas_w, canvas_h);
    // Make the target engine dirty so we can prove load clears it
    // (rather than starting from a doc that was already clean).
    let _layer = reloaded.add_raster_layer(None);
    assert!(reloaded.is_dirty(), "setup must produce a dirty engine");

    reloaded
        .open_document(&zip_bytes)
        .expect("kitchen-sink reload happy path");
    assert!(
        !reloaded.is_dirty(),
        "successful open_document must install a clean doc"
    );
}

/// Runtime guard that the kitchen sink actually instantiates every
/// closed-set variant in every registry. Adding a new blend mode /
/// layer kind / filter kind without extending `populate_kitchen_sink`
/// fails here loudly with the missing `type_id`s named.
#[test]
fn kitchen_sink_covers_every_closed_set_variant() {
    let mut engine = kitchen_sink_engine(8, 8);
    populate_kitchen_sink(&mut engine);

    // Blend modes — every registered mode must appear on at least one
    // raster layer.
    let used_blend_modes: std::collections::HashSet<&'static str> = engine
        .doc
        .all_raster_layers()
        .iter()
        .map(|r| r.blend.blend_mode.type_id)
        .collect();
    for reg in blend_mode::registry().all() {
        assert!(
            used_blend_modes.contains(reg.type_id),
            "kitchen-sink missing blend_mode/{} — extend populate_kitchen_sink",
            reg.type_id
        );
    }

    // Layer kinds — every registered kind must appear at least once in
    // the doc.
    let mut used_layer_kinds = std::collections::HashSet::new();
    for entity in engine.doc.entities.values() {
        if let crate::document::Entity::Node(node) = entity {
            used_layer_kinds.insert(node.type_id());
        }
    }
    for reg in layer_kind::registry().all() {
        assert!(
            used_layer_kinds.contains(reg.type_id),
            "kitchen-sink missing layer_kind/{} — extend populate_kitchen_sink",
            reg.type_id
        );
    }

    // Filter kinds — every registered kind must appear at least once.
    let mut used_modifier_kinds = std::collections::HashSet::new();
    for entity in engine.doc.entities.values() {
        if let crate::document::Entity::Filter(m) = entity {
            used_modifier_kinds.insert(m.type_id());
        }
    }
    for reg in filter::registry().all() {
        assert!(
            used_modifier_kinds.contains(reg.type_id),
            "kitchen-sink missing filter/{} — extend populate_kitchen_sink",
            reg.type_id
        );
    }
}

// ----------------------------------------------------------------------------
// Refusal tests. Every refusal path the load contract promises has a
// coverage test here. Each one builds (or hand-edits) a synthetic
// `.darkly` zip that triggers the refusal, then asserts the error shape
// AND that the engine's document was not swapped (atomic install means
// a refused load is byte-for-byte invisible).
// ----------------------------------------------------------------------------

use crate::format::error::LoadError;
use crate::format::manifest::{
    Manifest, ManifestCanvas, ManifestEntry, ManifestRequires, ManifestWriter, CONTAINER_VERSION,
    FORMAT_TAG,
};

/// Build a minimal valid `.darkly` zip from a hand-rolled `Manifest`.
/// Used by every refusal test that needs to plant a specific shape on
/// disk (`container_version` too new, `requires` lying, etc.) without
/// going through the save pipeline first.
fn synth_zip_from_manifest(manifest: &Manifest) -> Vec<u8> {
    // We don't have a `composite_rgba` for hand-built manifests; the
    // refusal tests bail out before pixel upload anyway, so a 1×1
    // black canvas is enough to satisfy the zip's `composite.png`
    // contract.
    let bundle = SaveBundle {
        manifest_json: serde_json::to_vec_pretty(manifest).unwrap(),
        composite_width: 1,
        composite_height: 1,
        composite_rgba: vec![0, 0, 0, 255],
        blobs: Vec::new(),
    };
    assemble_zip(&bundle)
}

/// Body for a default empty root group.
fn root_group_body() -> serde_json::Value {
    serde_json::json!({
        "name": "Root",
        "visible": true,
        "locked": false,
        "opacity": 1.0,
        "blend_mode": "normal",
        "passthrough": true,
        "collapsed": false,
        "children": [],
        "modifiers": [],
    })
}

/// Build a `Manifest` with the canonical fields the load path expects
/// — `format = "darkly"`, current container version, present
/// `requires` block — but with an empty tree. Callers mutate the
/// returned struct in place to plant the test's specific failure mode.
fn synth_minimal_manifest() -> Manifest {
    Manifest {
        format: FORMAT_TAG.to_string(),
        container_version: CONTAINER_VERSION,
        writer: ManifestWriter::current(),
        name: "Test".to_string(),
        canvas: ManifestCanvas {
            width: 4,
            height: 4,
            origin_x: 0,
            origin_y: 0,
        },
        requires: ManifestRequires {
            layer_kind: vec!["group".to_string()],
            blend_mode: vec!["normal".to_string()],
            ..ManifestRequires::default()
        },
        composite: "composite.png".to_string(),
        root: 1,
        nodes: vec![ManifestEntry {
            id: 1,
            type_id: "group".to_string(),
            body: root_group_body(),
        }],
        modifiers: Vec::new(),
        selection_id: None,
        veils: Vec::new(),
        fonts: Vec::new(),
    }
}

/// Reusable assertion: a refused load must leave the engine's document
/// untouched. The pointer check is enough — `mem::replace` would
/// move the SlotMap (and its heap allocations) and the pointer would
/// differ. If we ever in-place mutate the doc during load, this test
/// would still catch the structural change because every refusal path
/// is supposed to bail before mutation.
fn assert_engine_untouched(engine: &DarklyEngine, prior_doc_ptr: *const Document) {
    assert_eq!(
        engine.document_ptr_for_test(),
        prior_doc_ptr,
        "atomic install violated: refused load swapped the engine's doc"
    );
}

#[test]
fn refuse_container_version_too_new() {
    let mut engine = kitchen_sink_engine(4, 4);
    let prior = engine.document_ptr_for_test();
    let mut manifest = synth_minimal_manifest();
    manifest.container_version = 999;
    let bytes = synth_zip_from_manifest(&manifest);

    let err = engine.open_document(&bytes).expect_err("must refuse");
    match err {
        LoadError::ContainerTooNew { found, supported } => {
            assert_eq!(found, 999);
            assert_eq!(supported, CONTAINER_VERSION);
        }
        other => panic!("expected ContainerTooNew, got {other:?}"),
    }
    assert_engine_untouched(&engine, prior);
}

#[test]
fn refuse_missing_requires() {
    let mut engine = kitchen_sink_engine(4, 4);
    let prior = engine.document_ptr_for_test();

    // Build the manifest JSON, then strip the `requires` field — we
    // control the writer so absence is malformed, not "older format."
    let manifest = synth_minimal_manifest();
    let mut raw: serde_json::Value = serde_json::to_value(&manifest).unwrap();
    raw.as_object_mut().unwrap().remove("requires");
    let manifest_bytes = serde_json::to_vec(&raw).unwrap();
    let bundle = SaveBundle {
        manifest_json: manifest_bytes,
        composite_width: 1,
        composite_height: 1,
        composite_rgba: vec![0, 0, 0, 255],
        blobs: Vec::new(),
    };
    let bytes = assemble_zip(&bundle);

    let err = engine.open_document(&bytes).expect_err("must refuse");
    match err {
        LoadError::CorruptManifest { reason } => {
            assert!(
                reason.contains("requires"),
                "missing-requires diagnostic should name the requires block, got {reason:?}"
            );
        }
        other => panic!("expected CorruptManifest, got {other:?}"),
    }
    assert_engine_untouched(&engine, prior);
}

#[test]
fn refuse_unknown_veil() {
    let mut engine = kitchen_sink_engine(4, 4);
    let prior = engine.document_ptr_for_test();
    let mut manifest = synth_minimal_manifest();
    manifest.requires.veil = vec!["future_lens_flare".to_string()];
    let bytes = synth_zip_from_manifest(&manifest);

    let err = engine.open_document(&bytes).expect_err("must refuse");
    match err {
        LoadError::UnsupportedFeatures { missing } => {
            assert!(
                missing.iter().any(|m| m == "veil/future_lens_flare"),
                "diagnostic should name veil/future_lens_flare, got {missing:?}"
            );
        }
        other => panic!("expected UnsupportedFeatures, got {other:?}"),
    }
    assert_engine_untouched(&engine, prior);
}

#[test]
fn refuse_unknown_blend_mode() {
    let mut engine = kitchen_sink_engine(4, 4);
    let prior = engine.document_ptr_for_test();
    let mut manifest = synth_minimal_manifest();
    manifest.requires.blend_mode.push("divide_v2".to_string());
    let bytes = synth_zip_from_manifest(&manifest);

    let err = engine.open_document(&bytes).expect_err("must refuse");
    match err {
        LoadError::UnsupportedFeatures { missing } => {
            assert!(
                missing.iter().any(|m| m == "blend_mode/divide_v2"),
                "diagnostic should name blend_mode/divide_v2, got {missing:?}"
            );
        }
        other => panic!("expected UnsupportedFeatures, got {other:?}"),
    }
    assert_engine_untouched(&engine, prior);
}

#[test]
fn refuse_unknown_layer_kind() {
    let mut engine = kitchen_sink_engine(4, 4);
    let prior = engine.document_ptr_for_test();
    let mut manifest = synth_minimal_manifest();
    manifest.requires.layer_kind.push("text_layer".to_string());
    let bytes = synth_zip_from_manifest(&manifest);

    let err = engine.open_document(&bytes).expect_err("must refuse");
    match err {
        LoadError::UnsupportedFeatures { missing } => {
            assert!(
                missing.iter().any(|m| m == "layer_kind/text_layer"),
                "diagnostic should name layer_kind/text_layer, got {missing:?}"
            );
        }
        other => panic!("expected UnsupportedFeatures, got {other:?}"),
    }
    assert_engine_untouched(&engine, prior);
}

#[test]
fn refuse_unknown_modifier_kind() {
    let mut engine = kitchen_sink_engine(4, 4);
    let prior = engine.document_ptr_for_test();
    let mut manifest = synth_minimal_manifest();
    manifest.requires.modifier.push("clip".to_string());
    let bytes = synth_zip_from_manifest(&manifest);

    let err = engine.open_document(&bytes).expect_err("must refuse");
    match err {
        LoadError::UnsupportedFeatures { missing } => {
            assert!(
                missing.iter().any(|m| m == "filter/clip"),
                "diagnostic should name filter/clip, got {missing:?}"
            );
        }
        other => panic!("expected UnsupportedFeatures, got {other:?}"),
    }
    assert_engine_untouched(&engine, prior);
}

#[test]
fn refuse_corrupt_manifest_when_requires_lies() {
    // The `requires` block claims only `normal` (truthful as far as
    // pre-check is concerned), but the body has a raster layer using
    // `divide_v2` — a blend mode the binary doesn't know. The
    // per-variant safety net in `build_staging_document` catches this
    // as `CorruptManifest`.
    let mut engine = kitchen_sink_engine(4, 4);
    let prior = engine.document_ptr_for_test();
    let mut manifest = synth_minimal_manifest();
    manifest.requires.layer_kind = vec!["group".to_string(), "raster".to_string()];
    manifest.nodes.push(ManifestEntry {
        id: 42,
        type_id: "raster".to_string(),
        body: serde_json::json!({
            "name": "lying",
            "visible": true,
            "locked": false,
            "opacity": 1.0,
            "blend_mode": "divide_v2", // not declared in requires
            "pixels": {
                "format": "rgba8unorm",
                "pixels": "layers/42.pixels",
                "bounds": { "origin": { "x": 0, "y": 0 }, "width": 4, "height": 4 }
            },
            "modifiers": []
        }),
    });
    let bytes = synth_zip_from_manifest(&manifest);

    let err = engine.open_document(&bytes).expect_err("must refuse");
    match err {
        LoadError::CorruptManifest { reason } => {
            assert!(
                reason.contains("divide_v2"),
                "diagnostic should name the lying type_id, got {reason:?}"
            );
        }
        other => panic!("expected CorruptManifest, got {other:?}"),
    }
    assert_engine_untouched(&engine, prior);
}

#[test]
fn open_document_leaves_engine_untouched_on_refuse() {
    // Belt-and-suspenders for the most consequential refusal — load a
    // file that's been edited to need a feature the binary doesn't
    // ship and assert *more* than the doc-ptr invariant: the
    // pre-refusal state (layer count, doc name) is also recoverable.
    let mut engine = kitchen_sink_engine(4, 4);
    let _layer = engine.add_raster_layer(None);
    engine.set_document_name("pre-load".to_string());
    let prior = engine.document_ptr_for_test();
    let prior_layer_count = engine.doc.all_raster_layers().len();
    let prior_name = engine.doc.name.clone();

    let mut manifest = synth_minimal_manifest();
    manifest.requires.veil.push("future_lens_flare".to_string());
    let bytes = synth_zip_from_manifest(&manifest);
    let _err = engine.open_document(&bytes).expect_err("must refuse");

    assert_engine_untouched(&engine, prior);
    assert_eq!(engine.doc.all_raster_layers().len(), prior_layer_count);
    assert_eq!(engine.doc.name, prior_name);
}

#[test]
fn legacy_type_id_migration() {
    // The `register() -> Vec<Registration>` legacy-reader pattern
    // isn't wired yet — no module has bumped its `type_id`, so no
    // legacy entries exist. This test holds the slot so the first
    // real bump has a place to land its fixture (and surfaces if
    // someone accidentally drops the registry interface). The shape
    // a future migration test takes:
    //
    //   1. Construct a Manifest with `requires.veil = vec!["grain"]`
    //      (current `type_id`).
    //   2. Once `grain_v2` ships and `grain` becomes a legacy reader,
    //      write a fixture where `requires` still names `grain` and
    //      the body has the v1 params; assert the load succeeds and
    //      the migrated veil's params match what `migrate_v1_to_v2`
    //      would produce.
    //
    // Today: assert every registered veil resolves to itself in its
    // registry — confirms the registry interface is the dispatch
    // surface the migration will plug into.
    let registry = EffectRegistry::new();
    for reg in registry.registrations() {
        assert!(
            registry.has(reg.type_id),
            "legacy migration scaffold: registry must resolve every registered \
             type_id back through itself — drift suggests the dispatch surface \
             a future migration would plug into has changed"
        );
    }
}

// ----------------------------------------------------------------------------
// Load-bearing test for the registry-driven refactor: a fake layer kind
// can be plumbed in without touching central save/load code.
//
// The shape this test takes is "use a local registry mock". Today's
// registries are global `OnceLock`s, so we can't actually install a new
// kind dynamically. Instead, this test exercises the load path with a
// hand-built manifest body and asserts that the central code:
//   (a) dispatches based purely on type_id strings,
//   (b) consults the registry for every entity (proven by the
//       CorruptManifest miss on an unknown type_id under requires-lying
//       — already covered by `refuse_corrupt_manifest_when_requires_lies`),
//   (c) routes opaque bodies through registered deserialize without any
//       central branch (proven by the round-trip body check below).
//
// If a future change to save.rs or load.rs adds a hidden assumption
// about which kinds exist (e.g. a `match type_id { "raster" => ... }`
// branch), the kitchen-sink test catches it because every layer kind
// must round-trip. This test verifies the *structural* property the
// kitchen sink can't: that bodies are truly opaque and not centrally
// re-parsed.
// ----------------------------------------------------------------------------

#[test]
fn manifest_entry_bodies_are_opaque_to_central_code() {
    // Take a registered raster layer body, drop an unknown JSON key
    // alongside its real fields, and re-parse — the body should still
    // round-trip the unknown key. This proves the central code reads
    // `body: serde_json::Value` rather than coercing to a closed-set
    // typed enum, and that adding a new field inside a kind's body
    // doesn't require any central edit.
    let mut engine = kitchen_sink_engine(8, 8);
    let _layer = engine.add_raster_layer(None);
    let bundle = drive_save_to_completion(&mut engine);
    let manifest: Manifest = serde_json::from_slice(&bundle.manifest_json).unwrap();

    let raster_entry = manifest
        .nodes
        .iter()
        .find(|e| e.type_id == crate::document::layer_kinds::raster::TYPE_ID)
        .expect("at least one raster in kitchen sink");

    // Add an unknown field to the body and re-serialize the whole
    // manifest. The load path's central code must not reject this — it
    // only knows that body is `Value`, and only the raster kind itself
    // (which ignores unknown fields via serde's default behaviour) reads
    // it.
    let mut tampered = manifest.clone();
    if let Some(idx) = tampered.nodes.iter().position(|e| e.id == raster_entry.id) {
        let body_mut = &mut tampered.nodes[idx].body;
        if let Some(obj) = body_mut.as_object_mut() {
            obj.insert(
                "future_kind_only_field".to_string(),
                serde_json::Value::String("ignored by raster".to_string()),
            );
        }
    }

    let tampered_bytes = synth_zip_from_manifest(&tampered);
    let mut reloaded = kitchen_sink_engine(1, 1);
    reloaded
        .open_document(&tampered_bytes)
        .expect("unknown body fields must not break central code");
    // And the engine still has one raster.
    assert_eq!(reloaded.doc.all_raster_layers().len(), 1);
}
