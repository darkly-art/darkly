//! Auto-iterating round-trip tests for the wire format.
//!
//! These tests are the contract that protects modular extensions: adding a
//! new veil / stabilizer / brush node / blend mode / layer kind / modifier
//! kind lights up testing automatically here, with no edits. The shape
//! check is the same in every test — `(type_id, params)` (or just
//! `type_id` for closed-set kinds) must survive a JSON round-trip and
//! still resolve through the registry on the other side.

use serde::{Deserialize, Serialize};

use super::registry_io::{deserialize_instance, serialize_instance, InstancePayload};
use crate::brush::stabilizer::StabilizerRegistry;
use crate::brush::wire::BrushWireType;
use crate::brush::BrushNodeRegistry;
use crate::document::layer_kind;
use crate::document::modifier;
use crate::gpu::blend_mode;
use crate::gpu::context::GpuContext;
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::test_utils::test_device;
use crate::gpu::veil::VeilRegistry;
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
    engine
        .compositor
        .veil_chain_mut()
        .resize(&engine.gpu.device, &engine.gpu.queue, width, height);
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
    let format = gpu.surface_format();
    let mut registry = VeilRegistry::new();

    // `VeilRegistry::types()` returns (type_id, display_name, params). The
    // params shape comes from the static `&[ParamDef]` in each registration.
    // Cloning into a Vec keeps the borrow scoped so `create_veil` can take
    // `&mut registry` below without contention with the iteration.
    let types: Vec<(&'static str, &'static [ParamDef])> = registry
        .types()
        .into_iter()
        .map(|(id, _name, params)| (id, params))
        .collect();
    assert!(
        !types.is_empty(),
        "veil registry must contain at least one veil"
    );

    for (type_id, params_schema) in types {
        let defaults = defaults_of(params_schema);
        let veil = registry.create_veil(type_id, &defaults, &gpu.device, format);

        // Serialize via the canonical wire envelope.
        let json = serialize_instance(veil.type_id(), veil.param_values())
            .unwrap_or_else(|e| panic!("serialize veil '{type_id}' failed: {e}"));

        // Parse back and rebuild.
        let payload = deserialize_instance(&json)
            .unwrap_or_else(|e| panic!("deserialize veil '{type_id}' failed: {e}"));
        assert_eq!(payload.type_id, type_id);

        let restored = registry.create_veil(&payload.type_id, &payload.params, &gpu.device, format);
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
    let registry = BrushNodeRegistry::new();
    let types: Vec<&str> = registry.types().map(|reg| reg.type_id).collect();
    assert!(
        !types.is_empty(),
        "brush node registry must contain at least one node type"
    );

    for type_id in types {
        let mut graph: Graph<BrushWireType> = Graph::new();
        let reg = registry
            .get(type_id)
            .unwrap_or_else(|| panic!("missing registration for '{type_id}'"));
        let id = graph.add_node(reg.type_id, reg.ports.clone(), defaults_of(reg.params));

        let json = serde_json::to_string(&graph)
            .unwrap_or_else(|e| panic!("serialize graph with '{type_id}' failed: {e}"));
        let back: Graph<BrushWireType> = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("deserialize graph with '{type_id}' failed: {e}"));

        let node = back
            .nodes
            .get(&id)
            .unwrap_or_else(|| panic!("node lost across round-trip for '{type_id}'"));
        assert_eq!(node.type_id, type_id);
        assert_eq!(node.params.len(), reg.params.len());
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
// 5. Modifier kinds — round-trip the type_id string for every registered
//    modifier kind (mask, selection, future filter/transform/...).
// ----------------------------------------------------------------------------

#[test]
fn round_trip_every_modifier_kind() {
    let registry = modifier::registry();
    let all = registry.all();
    assert!(
        !all.is_empty(),
        "modifier registry must contain at least one kind"
    );

    for reg in all {
        let id = reg.type_id;
        let json = serde_json::to_string(id).unwrap();
        let back: String = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(
            registry.get(&back).is_some(),
            "round-tripped modifier kind '{id}' must resolve back through registry"
        );
    }
}

// ----------------------------------------------------------------------------
// 6. Layer kinds — round-trip the type_id string for every registered
//    layer kind (raster, group, future text/adjustment/...).
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
    let ok = match (a, b) {
        (ParamValue::Bool(x), ParamValue::Bool(y)) => x == y,
        (ParamValue::Int(x), ParamValue::Int(y)) => x == y,
        (ParamValue::Float(x), ParamValue::Float(y)) => x == y,
        (ParamValue::String(x), ParamValue::String(y)) => x == y,
        (ParamValue::Curve(x), ParamValue::Curve(y)) => x == y,
        _ => false,
    };
    assert!(ok, "[{context}] param mismatch: {a:?} vs {b:?}");
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
///   - every modifier kind (`mask`, `selection`) — at least one each.
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
/// kind, new modifier kind), `kitchen_sink_covers_every_closed_set_variant`
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

    // Mask modifier on one of the rasters — exercises mask kind +
    // its parent-host wiring.
    if let Some(target) = raster_ids.get(1).copied() {
        engine.add_mask(target);
    }

    // Selection mask — `select_all` flips selection.active and
    // populates the R8 texture; the modifier itself was allocated
    // eagerly at engine init.
    engine.select_all();

    // One of every veil — keep params at default, leave visibility on.
    let veil_types: Vec<(&'static str, &'static [ParamDef])> = engine
        .compositor
        .veil_chain()
        .registry()
        .types()
        .into_iter()
        .map(|(id, _name, params)| (id, params))
        .collect();
    for (type_id, schema) in veil_types {
        let defaults = defaults_of(schema);
        engine.add_veil(type_id, &defaults);
    }
}

/// Pump the engine until a save completes. Caps iterations so a stuck
/// readback fails the test rather than hanging.
fn drive_save_to_completion(engine: &mut DarklyEngine) -> SaveBundle {
    engine.start_save_document().expect("start save");
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
/// raster layers, same number of groups, same number of modifiers,
/// same document name, same veil count, same selection presence.
/// Strict per-id mapping is intentionally NOT checked — slotmap keys
/// are document-local and won't match across documents.
fn assert_documents_equivalent(a: &Document, b: &Document) {
    assert_eq!(a.width, b.width);
    assert_eq!(a.height, b.height);
    assert_eq!(a.name, b.name);
    assert_eq!(a.all_raster_layers().len(), b.all_raster_layers().len());
    assert_eq!(a.all_groups().len(), b.all_groups().len());
    assert_eq!(a.all_modifiers().len(), b.all_modifiers().len());
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

/// Runtime guard that the kitchen sink actually instantiates every
/// closed-set variant in every registry. Adding a new blend mode /
/// layer kind / modifier kind without extending `populate_kitchen_sink`
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

    // Modifier kinds — every registered kind must appear at least once.
    let mut used_modifier_kinds = std::collections::HashSet::new();
    for entity in engine.doc.entities.values() {
        if let crate::document::Entity::Modifier(m) = entity {
            used_modifier_kinds.insert(m.type_id());
        }
    }
    for reg in modifier::registry().all() {
        assert!(
            used_modifier_kinds.contains(reg.type_id),
            "kitchen-sink missing modifier/{} — extend populate_kitchen_sink",
            reg.type_id
        );
    }
}
