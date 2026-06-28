//! Request/response protocol dispatch tests (Plan step 2 + step 7 verification).
//!
//! These run native (headless `GpuContext`) and prove the registry routes,
//! handlers decode/encode, and the binary side-channel round-trips. They
//! cannot reproduce the *browser* event-pump re-entrancy panic — that is the
//! manual browser repro's job (see plan Verification).
//!
//! Run with: `cargo test -p darkly --test protocol --features testing`

use darkly::engine::protocol::{ProtocolError, RequestRegistry, Response};
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use serde_json::json;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

#[test]
fn registry_routes_every_kind_and_rejects_unknown() {
    let reg = RequestRegistry::new();
    let kinds = reg.all_kinds();
    assert!(
        kinds.contains(&"add_raster"),
        "spike handler must be registered"
    );
    assert!(kinds.contains(&"add_void"));
    assert!(kinds.contains(&"layer_tree"));
    // `add_group` comes from a `#[handler]`-tagged engine method, aggregated by
    // build.rs — proves the macro registration path is wired into the registry.
    assert!(kinds.contains(&"add_group"));

    let mut engine = test_engine(64, 64);
    let err = reg
        .dispatch(&mut engine, "definitely_not_a_kind", json!({}), &[])
        .unwrap_err();
    assert_eq!(err, ProtocolError::unknown("definitely_not_a_kind"));
    assert_eq!(
        err.to_json(),
        json!({ "kind": "unknown_request", "message": "definitely_not_a_kind" })
    );
}

#[test]
fn add_raster_dispatch_adds_a_layer_and_returns_id() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);

    let before = engine.layer_tree().len();
    let resp = reg
        .dispatch(&mut engine, "add_raster", json!({ "anchor": null }), &[])
        .expect("add_raster dispatch");
    let id = resp.value.get("id").and_then(|v| v.as_u64());
    assert!(id.is_some(), "add_raster returns an id");
    assert!(resp.bytes.is_none(), "non-binary response carries no bytes");
    assert_eq!(engine.layer_tree().len(), before + 1, "a layer was added");
}

#[test]
fn add_void_unknown_type_returns_null_id() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);
    let resp = reg
        .dispatch(
            &mut engine,
            "add_void",
            json!({ "void_type": "not_a_void", "params": {}, "anchor": null }),
            &[],
        )
        .expect("add_void dispatch is infallible at the protocol level");
    // An unknown void type yields no layer: `Option<LayerId>` serializes the
    // `None` as a JSON `null`, not the old `-1` sentinel.
    assert!(resp.value.get("id").is_some_and(|v| v.is_null()));
}

#[test]
fn layer_tree_query_round_trips_to_an_array() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);
    let resp = reg
        .dispatch(&mut engine, "layer_tree", json!(null), &[])
        .expect("layer_tree dispatch");
    assert!(resp.value.is_array(), "layer_tree is a JSON array");
}

#[test]
fn bad_payload_is_a_protocol_error() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);
    // `anchor` is required and must be an integer.
    let err = reg
        .dispatch(&mut engine, "add_raster", json!({ "anchor": "nope" }), &[])
        .unwrap_err();
    assert!(matches!(err, ProtocolError::BadPayload(_)));
}

/// The TS `RequestKind` union is generated from the registry. This test
/// regenerates the expected file content and asserts it matches what's checked
/// in, so a new handler can't ship without the frontend's kind union learning
/// about it. Regenerate with `DARKLY_REGEN_TS=1`.
#[test]
fn request_kind_ts_union_is_in_sync() {
    let reg = RequestRegistry::new();
    let kinds = reg.all_kinds();

    let mut ts = String::new();
    ts.push_str("// @generated from RequestRegistry::all_kinds() — do not edit by hand.\n");
    ts.push_str(
        "// Regenerate: DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing\n\n",
    );
    ts.push_str("export type RequestKind =\n");
    for (i, k) in kinds.iter().enumerate() {
        let sep = if i == 0 { '=' } else { '|' };
        let _ = sep; // formatting handled below
        ts.push_str(&format!("    | '{k}'\n"));
    }
    ts.push_str("    ;\n\n");
    ts.push_str("export const REQUEST_KINDS: readonly RequestKind[] = [\n");
    for k in &kinds {
        ts.push_str(&format!("    '{k}',\n"));
    }
    ts.push_str("] as const;\n");

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../frontend/src/engine/protocol_gen.ts");

    if std::env::var("DARKLY_REGEN_TS").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &ts).unwrap();
        return;
    }

    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        actual, ts,
        "protocol_gen.ts is stale — run DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing"
    );
}

/// `LayerId` is wire-native: it serializes as a bare `u64` and round-trips
/// losslessly, so handlers can carry it directly instead of `to_ffi`/`from_ffi`
/// shimming. Regression guard for the typed-bridge coercion.
#[test]
fn layer_id_round_trips_through_json_as_a_number() {
    use darkly::layer::LayerId;
    let id = LayerId::from_ffi(0x0000_0002_0000_0007);
    let v = serde_json::to_value(id).unwrap();
    assert!(v.is_u64(), "LayerId serializes as a bare JSON number");
    let back: LayerId = serde_json::from_value(v).unwrap();
    assert_eq!(id, back, "LayerId round-trips losslessly");
}

/// `MoveTarget` deserializes straight from the `{ target_type, target_id }`
/// wire shape (adjacently-tagged enum), retiring the hand-written `&str →
/// variant` map. Includes the `#[serde(flatten)]` path the move handlers use.
#[test]
fn move_target_deserializes_from_wire_shape() {
    use darkly::document::MoveTarget;
    use darkly::layer::LayerId;
    let id = LayerId::from_ffi(42);
    let raw = id.to_ffi();

    let before: MoveTarget =
        serde_json::from_value(json!({ "target_type": "before", "target_id": raw })).unwrap();
    assert!(matches!(before, MoveTarget::Before(_)));
    let into_top: MoveTarget =
        serde_json::from_value(json!({ "target_type": "into_top", "target_id": raw })).unwrap();
    assert!(matches!(into_top, MoveTarget::IntoGroupTop(_)));
    let into_bottom: MoveTarget =
        serde_json::from_value(json!({ "target_type": "into_bottom", "target_id": raw })).unwrap();
    assert!(matches!(into_bottom, MoveTarget::IntoGroupBottom(_)));

    // Flattened into a sibling-bearing request, exactly as `move_layer` decodes.
    #[derive(serde::Deserialize)]
    struct MoveReq {
        id: LayerId,
        #[serde(flatten)]
        target: MoveTarget,
    }
    let req: MoveReq =
        serde_json::from_value(json!({ "id": raw, "target_type": "after", "target_id": raw }))
            .unwrap();
    assert_eq!(req.id, id);
    assert!(matches!(req.target, MoveTarget::After(_)));
}

/// `OrthoXform` deserializes from its `snake_case` name, retiring the per-handler
/// `axis`/`dir` → variant maps in the flip/rotate handlers.
#[test]
fn ortho_xform_deserializes_from_snake_case() {
    use darkly::gpu::ortho_transform::OrthoXform;
    let cases = [
        ("flip_h", OrthoXform::FlipH),
        ("flip_v", OrthoXform::FlipV),
        ("rot180", OrthoXform::Rot180),
        ("rot90_cw", OrthoXform::Rot90Cw),
        ("rot90_ccw", OrthoXform::Rot90Ccw),
    ];
    for (wire, expect) in cases {
        let x: OrthoXform = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(x, expect, "{wire} deserializes to {expect:?}");
    }
}

/// End-to-end through the registry for a `#[handler]`-generated handler: the
/// `move_layer` registration (emitted by the macro, aggregated by build.rs)
/// decodes `{ layer_id, target: { target_type, target_id } }` — `LayerId` plus
/// a nested `MoveTarget` param — and reorders without a protocol error. The
/// `()` return serializes to `null`.
#[test]
fn macro_move_layer_dispatch_reorders_via_nested_target() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);

    let a = reg
        .dispatch(&mut engine, "add_raster", json!({ "anchor": null }), &[])
        .unwrap()
        .value
        .get("id")
        .and_then(|v| v.as_u64())
        .expect("first raster id");
    let b = reg
        .dispatch(&mut engine, "add_raster", json!({ "anchor": null }), &[])
        .unwrap()
        .value
        .get("id")
        .and_then(|v| v.as_u64())
        .expect("second raster id");

    let resp = reg
        .dispatch(
            &mut engine,
            "move_layer",
            json!({ "layer_id": a, "target": { "target_type": "before", "target_id": b } }),
            &[],
        )
        .expect("macro-generated move_layer decodes LayerId + nested MoveTarget");
    assert!(resp.value.is_null(), "a `()` return serializes to null");
}

/// The macro's autoref response conversion produces the engine's *natural*
/// return shapes — no `{ id }`/`{ skipped }` envelope. `add_group` (`-> LayerId`)
/// is a bare number; `group_layers`'s `Err` (`Result<_, String>`) rejects as a
/// [`ProtocolError::Engine`], not a resolved `{ error }` value.
#[test]
fn macro_handlers_use_natural_return_shapes() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);

    let group = reg
        .dispatch(&mut engine, "add_group", json!({ "anchor": null }), &[])
        .expect("add_group dispatch");
    assert!(
        group.value.is_u64(),
        "add_group returns a bare LayerId number, not a {{ id }} envelope"
    );
    assert!(group.bytes.is_none());

    let err = reg
        .dispatch(&mut engine, "group_layers", json!({ "ids": [] }), &[])
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Engine(_)),
        "Result<_, String>::Err rejects as an engine error"
    );
}

#[test]
fn binary_side_channel_round_trips() {
    // Structural: Response::binary carries bytes verbatim out-of-band.
    let payload = vec![1u8, 2, 3, 4, 255];
    let resp = Response::binary(json!({ "len": payload.len() }), payload.clone());
    assert_eq!(resp.bytes, Some(payload));
    assert_eq!(resp.value.get("len").and_then(|v| v.as_u64()), Some(5));
}
