//! Per-channel input histogram for the Levels editor — an async compute
//! readback mirroring [`color_pick`](super::color_pick).
//!
//! `request_histogram` selects the target filter (or clears it with `id < 0`);
//! the compositor bins that filter's input on the next compose. `histogram_result`
//! returns the cached `8×256 u32` bytes (empty while pending), and
//! `has_pending_histogram` reports whether a dispatch is in flight.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;

#[derive(Deserialize)]
struct IdReq {
    id: f64,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "request_histogram",
            handle: |engine, payload, _b| {
                let r: IdReq = decode(payload)?;
                let target = (r.id >= 0.0).then(|| LayerId::from_ffi(r.id as u64));
                engine.set_histogram_target(target);
                Ok(Response::json(serde_json::Value::Null))
            },
        },
        RequestRegistration {
            kind: "histogram_result",
            handle: |engine, payload, _b| {
                let r: IdReq = decode(payload)?;
                let bytes = if r.id >= 0.0 {
                    engine.histogram(LayerId::from_ffi(r.id as u64))
                } else {
                    Vec::new()
                };
                Ok(Response::binary(serde_json::Value::Null, bytes))
            },
        },
        RequestRegistration {
            kind: "has_pending_histogram",
            handle: |engine, _payload, _b| {
                Ok(Response::json(serde_json::json!({
                    "value": engine.has_pending_histogram(),
                })))
            },
        },
    ]
}
