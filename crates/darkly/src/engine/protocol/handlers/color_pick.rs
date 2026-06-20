//! Color picking — async readback trigger plus cached-color queries.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::engine::PickSource;
use crate::layer::LayerId;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "pick_color",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    x: f32,
                    y: f32,
                    id: f64,
                }
                let r: Req = decode(payload)?;
                let source = if r.id >= 0.0 {
                    PickSource::Layer(LayerId::from_ffi(r.id as u64))
                } else {
                    PickSource::Merged
                };
                let color = engine.pick_color(r.x, r.y, source);
                Ok(Response::binary(serde_json::Value::Null, color.to_vec()))
            },
        },
        RequestRegistration {
            kind: "last_picked_color",
            handle: |engine, _payload, _b| {
                Ok(Response::binary(
                    serde_json::Value::Null,
                    engine.last_picked_color().to_vec(),
                ))
            },
        },
        RequestRegistration {
            kind: "has_pending_color_pick",
            handle: |engine, _payload, _b| {
                Ok(Response::json(serde_json::json!({
                    "value": engine.has_pending_color_pick(),
                })))
            },
        },
    ]
}
