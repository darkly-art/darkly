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
        .dispatch(&mut engine, "add_raster", json!({ "anchor": -1 }), &[])
        .expect("add_raster dispatch");
    let id = resp.value.get("id").and_then(|v| v.as_u64());
    assert!(id.is_some(), "add_raster returns an id");
    assert!(resp.bytes.is_none(), "non-binary response carries no bytes");
    assert_eq!(engine.layer_tree().len(), before + 1, "a layer was added");
}

#[test]
fn add_void_unknown_type_returns_minus_one() {
    let reg = RequestRegistry::new();
    let mut engine = test_engine(64, 64);
    let resp = reg
        .dispatch(
            &mut engine,
            "add_void",
            json!({ "void_type": "not_a_void", "params": {}, "anchor": -1 }),
            &[],
        )
        .expect("add_void dispatch is infallible at the protocol level");
    assert_eq!(resp.value.get("id").and_then(|v| v.as_i64()), Some(-1));
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

#[test]
fn binary_side_channel_round_trips() {
    // Structural: Response::binary carries bytes verbatim out-of-band.
    let payload = vec![1u8, 2, 3, 4, 255];
    let resp = Response::binary(json!({ "len": payload.len() }), payload.clone());
    assert_eq!(resp.bytes, Some(payload));
    assert_eq!(resp.value.get("len").and_then(|v| v.as_u64()), Some(5));
}
