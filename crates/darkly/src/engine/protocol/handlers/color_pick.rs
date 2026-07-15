//! Async color-pick trigger. The cached-color reads (`last_picked_color`,
//! `has_pending_color_pick`) are `#[handler]`-generated on `engine/rendering.rs`;
//! `pick_color` stays hand-written because its `id` field is an `f64`
//! negative-means-merged sentinel that resolves to a [`PickSource`] enum the
//! macro can't derive from a single wire field.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::engine::PickSource;
use crate::layer::LayerId;

/// `{ x, y, id }` — the canvas point to sample and the layer to sample from
/// (`id < 0` → the merged composite).
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct PickColorReq {
    pub x: f32,
    pub y: f32,
    pub id: f64,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("pick_color", |engine, payload, _b| {
            let r: PickColorReq = decode(payload)?;
            let source = if r.id >= 0.0 {
                PickSource::Layer(LayerId::from_ffi(r.id as u64))
            } else {
                PickSource::Merged
            };
            let color = engine.pick_color(r.x, r.y, source);
            Ok(Response::binary(serde_json::Value::Null, color.to_vec()))
        })
        // Fire-and-forget: the picked color is read back later via
        // `last_picked_color` after `has_pending_color_pick` clears.
        .post()
        .req::<PickColorReq>(),
    ]
}
