//! Stroke lifecycle requests — begin / continue / end.

use crate::engine::protocol::{decode, layer_id, RequestRegistration, Response};
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("begin_stroke", |engine, payload, _b| {
            engine.begin_stroke(layer_id(payload)?);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("stroke_to", |engine, payload, _b| {
            let op: crate::engine::types::StrokeOp = decode(payload)?;
            engine.stroke_to(op);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("end_stroke", |engine, _payload, _b| {
            engine.end_stroke();
            Ok(Response::empty())
        }),
    ]
}
