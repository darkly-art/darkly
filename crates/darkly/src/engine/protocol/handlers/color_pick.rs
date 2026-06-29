//! Async color-pick trigger. The cached-color reads (`last_picked_color`,
//! `has_pending_color_pick`) are `#[handler]`-generated on `engine/rendering.rs`;
//! `pick_color` stays hand-written because its `id` field is an `f64`
//! negative-means-merged sentinel that resolves to a [`PickSource`] enum the
//! macro can't derive from a single wire field.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::engine::PickSource;
use crate::layer::LayerId;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![RequestRegistration {
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
    }]
}
