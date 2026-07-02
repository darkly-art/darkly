//! Floating content (paste/transform) query that needs wire shaping the
//! `#[handler]` macro can't derive. The lifecycle verbs and the scalar queries
//! are `#[handler]`-generated on `engine/floating.rs`; only `floating_info`
//! stays here — its engine return is a composite `Option<(ox, oy, w, h, Transform)>`
//! tuple, and the wire wants a flat `{ ox, oy, w, h, mode, matrix }` object with
//! `mode`/`matrix` *derived* from the `Transform` (6 affine floats for `Basic`,
//! 9 homography floats for `Perspective`; the frontend's `liftMatrix` picks the
//! variant by `mode`).

use serde::Serialize;

use crate::engine::protocol::{RequestRegistration, Response};

/// Flat bounds + mode-tagged transform payload of the active floating content.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct FloatingInfoResp {
    pub ox: f32,
    pub oy: f32,
    pub w: f32,
    pub h: f32,
    pub mode: u32,
    pub matrix: Vec<f32>,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("floating_info", |engine, _payload, _b| {
            let value = match engine.floating_info() {
                Some((ox, oy, w, h, t)) => serde_json::to_value(FloatingInfoResp {
                    ox,
                    oy,
                    w,
                    h,
                    mode: t.mode_tag(),
                    matrix: t.wire_payload(),
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
