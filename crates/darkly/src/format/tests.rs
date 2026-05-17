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
