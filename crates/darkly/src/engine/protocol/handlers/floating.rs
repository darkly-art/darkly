//! Floating content (paste/transform) queries that need wire shaping the
//! `#[handler]` macro can't derive. The lifecycle verbs and the scalar queries
//! are `#[handler]`-generated on `engine/floating.rs`; only `floating_info`
//! stays here — its engine return is a composite `Option<(ox, oy, w, h, matrix)>`
//! tuple, and the wire wants a flat `{ ox, oy, w, h, matrix }` object.

use serde_json::json;

use crate::engine::protocol::{RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![RequestRegistration {
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
    }]
}
