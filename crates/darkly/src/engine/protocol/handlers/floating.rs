//! Floating content (paste/transform) lifecycle + queries.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, layer_id, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "update_floating_matrix",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    matrix: [f32; 6],
                }
                let r: Req = decode(payload)?;
                engine.update_floating_matrix(r.matrix);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "commit_floating",
            handle: |engine, _payload, _b| {
                engine.commit_floating();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "cancel_floating",
            handle: |engine, _payload, _b| {
                engine.cancel_floating();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "paste_in_place_floating",
            handle: |engine, payload, _b| {
                let ok = engine.paste_in_place_floating(layer_id(payload)?);
                Ok(Response::json(json!({ "ok": ok })))
            },
        },
        RequestRegistration {
            kind: "begin_transform",
            handle: |engine, payload, _b| {
                // Deferred: spawns a task that warms the selection cache /
                // awaits content bounds, then sets up the floating session and
                // resolves this request with `{ ok }`.
                engine.spawn_begin_transform(layer_id(payload)?);
                Ok(Response::deferred())
            },
        },
        RequestRegistration {
            kind: "has_floating",
            handle: |engine, _payload, _b| {
                Ok(Response::json(json!({ "value": engine.has_floating() })))
            },
        },
        RequestRegistration {
            kind: "floating_info",
            handle: |engine, _payload, _b| {
                let value = match engine.floating_info() {
                    Some((ox, oy, w, h, m)) => {
                        json!({ "ox": ox, "oy": oy, "w": w, "h": h, "matrix": m })
                    }
                    None => serde_json::Value::Null,
                };
                Ok(Response::json(value))
            },
        },
        RequestRegistration {
            kind: "floating_target_layer",
            handle: |engine, _payload, _b| {
                let id = engine
                    .floating_target_layer()
                    .map(|i| i.to_ffi() as i64)
                    .unwrap_or(-1);
                Ok(Response::json(json!({ "id": id })))
            },
        },
    ]
}
