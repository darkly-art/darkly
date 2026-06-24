//! Request/response protocol dispatch tests.
//!
//! These run native (headless `GpuContext`) and prove the registry routes,
//! handlers decode/encode, and the binary side-channel round-trips. They
//! cannot reproduce the *browser* event-pump re-entrancy panic — that is the
//! manual browser repro's job.
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

/// The frontend's typed engine client (`protocol_gen.ts`) is generated from the
/// registry: the `RequestKind` union, the transitive TS type declarations of
/// every request/response type, and a typed `EngineApi` mapping each kind to a
/// method. This test regenerates the expected content and asserts it matches
/// what's checked in, so a handler can't ship — or change a wire type — without
/// the frontend's generated client learning about it. Regenerate with
/// `DARKLY_REGEN_TS=1`.
///
/// Gated on `ts-export` (implied by `testing`) because emitting the type
/// declarations requires the `#[derive(ts_rs::TS)]` impls, which release / WASM
/// builds deliberately omit. CI always runs `--features darkly/testing`, so the
/// guard always fires there.
#[cfg(feature = "ts-export")]
#[test]
fn protocol_gen_ts_is_in_sync() {
    let reg = RequestRegistry::new();
    let ts = darkly_protocol_gen::generate(&reg);

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

/// The typed-client generator. Kept in one place so the regen path and the
/// compare path emit byte-identical output.
#[cfg(feature = "ts-export")]
mod darkly_protocol_gen {
    use darkly::engine::protocol::ts_export::{config, TsCollector};
    use darkly::engine::protocol::RequestRegistry;

    /// Map a ts-rs type *name* to the TS type expression we want at use sites:
    /// ts-rs renders `serde_json::Value` as `JsonValue` (aliased to `any`) and
    /// `()` as `null`; the method surface reads better with `any` / `void`.
    fn use_site(name: &str) -> &str {
        match name {
            "JsonValue" => "any",
            "null" => "void",
            other => other,
        }
    }

    pub fn generate(reg: &RequestRegistry) -> String {
        let cfg = config();
        let kinds = reg.all_kinds();
        let sigs = reg.ts_signatures();

        // Transitive closure of every named request/response declaration.
        let mut collector = TsCollector::default();
        for (_kind, sig) in &sigs {
            (sig.req.collect)(&mut collector);
            (sig.resp.collect)(&mut collector);
        }
        let decls: Vec<&str> = collector.declarations().collect();

        let mut ts = String::new();
        ts.push_str("// @generated from RequestRegistry — do not edit by hand.\n");
        ts.push_str(
            "// Regenerate: DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing\n\n",
        );

        // 1. Wire type declarations (named structs/enums, transitively).
        for decl in &decls {
            ts.push_str(decl);
            ts.push('\n');
        }
        if !decls.is_empty() {
            ts.push('\n');
        }

        // 2. The wire-key union + runtime list.
        ts.push_str("export type RequestKind =\n");
        for k in &kinds {
            ts.push_str(&format!("    | '{k}'\n"));
        }
        ts.push_str("    ;\n\n");
        ts.push_str("export const REQUEST_KINDS: readonly RequestKind[] = [\n");
        for k in &kinds {
            ts.push_str(&format!("    '{k}',\n"));
        }
        ts.push_str("] as const;\n\n");

        // 3. The typed request surface. `Engine` implements this via declaration
        //    merging; each method wraps `send(kind, payload, bytes?)`.
        ts.push_str(
            "/** Typed request surface — one method per registered kind, generated\n \
             *  from each handler's request/response types. `Engine` implements this\n \
             *  (declaration merging) so callers write `engine.copy(req)` rather than\n \
             *  `engine.send('copy', req)`. */\n",
        );
        ts.push_str("export interface EngineApi {\n");
        for (k, sig) in &sigs {
            let req = use_site(&(sig.req.name)(&cfg)).to_string();
            let resp = use_site(&(sig.resp.name)(&cfg)).to_string();

            // Request argument: void payload -> no `req`; `any` placeholder ->
            // optional; a concrete type -> required. A binary-in request appends
            // a required `bytes` argument.
            let mut params: Vec<String> = Vec::new();
            match req.as_str() {
                "void" => {}
                "any" => params.push("req?: any".to_string()),
                other => params.push(format!("req: {other}")),
            }
            if sig.req_bytes {
                params.push("bytes: Uint8Array".to_string());
            }

            // Response: void stays void; a binary-out response intersects the
            // JSON value with the `bytes` field the transport attaches.
            let ret = if sig.resp_bytes {
                match resp.as_str() {
                    "void" => "{ bytes: Uint8Array }".to_string(),
                    other => format!("{other} & {{ bytes: Uint8Array }}"),
                }
            } else {
                resp
            };

            ts.push_str(&format!(
                "    {k}({}): Promise<{ret}>;\n",
                params.join(", ")
            ));
        }
        ts.push_str("}\n");
        ts
    }
}

#[test]
fn binary_side_channel_round_trips() {
    // Structural: Response::binary carries bytes verbatim out-of-band.
    let payload = vec![1u8, 2, 3, 4, 255];
    let resp = Response::binary(json!({ "len": payload.len() }), payload.clone());
    assert_eq!(resp.bytes, Some(payload));
    assert_eq!(resp.value.get("len").and_then(|v| v.as_u64()), Some(5));
}

/// Regression: a one-shot readback op (`copy`) defers — the request that kicked
/// the readback is the request that resolves with the result, on a *later*
/// drain, with no separate `poll_*` round-trip. `copy` spawns a task on the
/// engine host's executor; the dispatch emits no outcome (`Response::deferred`),
/// and the task resolves the originating request once the host drives it.
#[test]
fn deferred_copy_resolves_originating_request_on_a_later_drain() {
    use darkly::engine::host::EngineHost;
    use darkly::engine::protocol::Transport;
    use darkly::engine::ClipboardExport;

    let mut engine = test_engine(32, 32);
    // `paste_image` allocates the raster layer's GPU texture and uploads
    // pixels, so the copy readback has something real to read back.
    let rgba = vec![128u8; (32 * 32 * 4) as usize];
    let layer = engine.paste_image(32, 32, &rgba, 0, 0, None);

    let host = EngineHost::adopt(engine);
    let transport = Transport::new();
    const REQ: u64 = 4242;
    transport.enqueue(REQ, "copy", json!({ "id": layer.to_ffi() }), Vec::new());

    // Dispatch `copy` (without driving its task): the deferred handler spawns
    // the task and emits no outcome for REQ — the poll model would have resolved
    // it now, with null.
    let first = host.with(|e| transport.drain_with(e));
    assert!(
        !first.iter().any(|o| o.id == REQ),
        "copy must defer — no outcome on the drain that kicked the readback"
    );

    // Drive the task to completion, then drain again: the originating request
    // resolves with the ClipboardExport — no `poll_copy_result` hop.
    host.pump_until_idle();
    let second = host.with(|e| transport.drain_with(e));
    let outcome = second
        .iter()
        .find(|o| o.id == REQ)
        .expect("copy resolves the originating request on a later drain");
    let resp = outcome
        .result
        .as_ref()
        .expect("copy resolved, not rejected");
    let export: ClipboardExport =
        serde_json::from_value(resp.value.clone()).expect("copy response is a ClipboardExport");
    assert_eq!(export.width, 32, "copied region spans the full canvas");
    assert_eq!(export.height, 32);
}
