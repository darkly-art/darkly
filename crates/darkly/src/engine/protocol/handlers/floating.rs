//! Floating content (paste/transform) queries that need wire shaping the
//! `#[handler]` macro can't derive. The lifecycle verbs and the scalar queries
//! are `#[handler]`-generated on `engine/floating.rs`; only `floating_info`
//! stays here — its engine return is a composite `Option<(ox, oy, w, h, matrix)>`
//! tuple, and the wire wants a flat `{ ox, oy, w, h, matrix }` object.

use serde::Serialize;

use crate::engine::protocol::{RequestRegistration, Response};

/// Flat bounds + affine of the active floating content.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct FloatingInfoResp {
    pub ox: f32,
    pub oy: f32,
    pub w: f32,
    pub h: f32,
    pub matrix: [f32; 6],
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("floating_info", |engine, _payload, _b| {
            let value = match engine.floating_info() {
                Some((ox, oy, w, h, matrix)) => serde_json::to_value(FloatingInfoResp {
                    ox,
                    oy,
                    w,
                    h,
                    matrix,
                })
                .map_err(crate::engine::protocol::bad_payload)?,
                None => serde_json::Value::Null,
            };
            Ok(Response::json(value))
        })
        .send()
        .resp::<Option<FloatingInfoResp>>(),
    ]
}
