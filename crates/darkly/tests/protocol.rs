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
    assert!(kinds.contains(&"add_raster"));
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
    // `#[handler]`-derived: the natural `LayerId` return is a bare JSON number,
    // not a `{ id }` envelope.
    assert!(
        resp.value.as_u64().is_some(),
        "add_raster returns a bare id"
    );
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
    // An unknown void type yields no layer: the natural `Option<LayerId>` return
    // serializes `None` as a bare JSON `null` (no `{ id }` envelope, no `-1`).
    assert!(resp.value.is_null());
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

/// The typed TS client (`protocol_gen.ts`) is generated from the registry:
/// per-kind `Req`/`Resp` interfaces (ts-rs), the `RequestKind` union, and the
/// `EngineApi` + `makeApi` surface. This test regenerates the expected file and
/// asserts it matches what's checked in, so a handler can't drift from the
/// frontend's typed client. Regenerate with `DARKLY_REGEN_TS=1`. Needs the
/// `ts-export` feature (ts-rs is off the production/wasm path).
#[cfg(feature = "ts-export")]
#[test]
fn protocol_gen_ts_is_in_sync() {
    let ts = darkly::engine::protocol::codegen::generate_protocol_ts();

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
        "protocol_gen.ts is stale — run DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing,ts-export"
    );
}

/// Every registered kind must appear as an `EngineApi` method — no untyped gap.
#[cfg(feature = "ts-export")]
#[test]
fn every_kind_has_an_engine_api_method() {
    let reg = RequestRegistry::new();
    let ts = darkly::engine::protocol::codegen::generate_protocol_ts();
    for kind in reg.all_kinds() {
        let method = darkly::engine::protocol::codegen::camel_case(kind);
        assert!(
            ts.contains(&format!("{method}(")),
            "kind `{kind}` has no generated EngineApi method `{method}`"
        );
    }
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
/// decodes `{ id, target: { target_type, target_id } }` — `LayerId` plus a
/// nested `MoveTarget` param — and reorders without a protocol error. The `()`
/// return serializes to `null`.
#[test]
fn macro_move_layer_dispatch_reorders_via_nested_target() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);

    let a = reg
        .dispatch(&mut engine, "add_raster", json!({ "anchor": null }), &[])
        .unwrap()
        .value
        .as_u64()
        .expect("first raster id");
    let b = reg
        .dispatch(&mut engine, "add_raster", json!({ "anchor": null }), &[])
        .unwrap()
        .value
        .as_u64()
        .expect("second raster id");

    let resp = reg
        .dispatch(
            &mut engine,
            "move_layer",
            json!({ "id": a, "target": { "target_type": "before", "target_id": b } }),
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

#[test]
fn poll_recording_frame_reports_canvas_dims_when_empty() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);
    let resp = reg
        .dispatch(&mut engine, "poll_recording_frame", json!(null), &[])
        .expect("poll_recording_frame dispatch");
    // No frame pending, but the envelope still carries the live canvas
    // dimensions — the poll doubles as the frontend's resize signal.
    assert_eq!(resp.value["canvasWidth"], json!(64));
    assert_eq!(resp.value["canvasHeight"], json!(64));
    assert!(resp.value["frame"].is_null());
    assert!(resp.bytes.is_none());
}
