//! Per-channel input histogram for the Levels editor — an async compute
//! readback mirroring [`color_pick`](super::color_pick).
//!
//! `request_histogram` selects the target filter (or clears it with `id < 0`);
//! the compositor bins that filter's input on the next compose. `histogram_result`
//! returns the cached `8×256 u32` bytes (empty while the readback is pending).

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;

/// `{ id }` — the filter layer to histogram (`id < 0` clears the target).
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct HistogramReq {
    pub id: f64,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        // Fire-and-forget: select which filter's input to bin (`id < 0` clears).
        RequestRegistration::new("request_histogram", |engine, payload, _b| {
            let r: HistogramReq = decode(payload)?;
            let target = (r.id >= 0.0).then(|| LayerId::from_ffi(r.id as u64));
            engine.set_histogram_target(target);
            Ok(Response::json(serde_json::Value::Null))
        })
        .post()
        .req::<HistogramReq>(),
        // Awaited: the cached 8×256 u32 histogram bytes (empty while pending).
        RequestRegistration::new("histogram_result", |engine, payload, _b| {
            let r: HistogramReq = decode(payload)?;
            let bytes = if r.id >= 0.0 {
                engine.histogram(LayerId::from_ffi(r.id as u64))
            } else {
                Vec::new()
            };
            Ok(Response::binary(serde_json::Value::Null, bytes))
        })
        .send()
        .req::<HistogramReq>()
        .bytes_out(),
    ]
}
