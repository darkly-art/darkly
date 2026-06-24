//! Painting requests — background fills.

use serde::Deserialize;

use crate::engine::protocol::{decode, layer_id, RequestRegistration, Response};
use crate::layer::LayerId;
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("fill_background", |engine, payload, _b| {
            engine.fill_background(layer_id(payload)?);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("fill_background_color", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                rgba: [u8; 4],
            }
            let r: Req = decode(payload)?;
            engine.fill_background_color(LayerId::from_ffi(r.id), r.rgba);
            Ok(Response::empty())
        }),
    ]
}
