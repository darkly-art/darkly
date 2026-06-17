//! Stroke lifecycle requests — begin / continue / end.

use crate::engine::protocol::{decode, layer_id, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "begin_stroke",
            handle: |engine, payload, _b| {
                engine.begin_stroke(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "stroke_to",
            handle: |engine, payload, _b| {
                let op: crate::engine::types::StrokeOp = decode(payload)?;
                engine.stroke_to(op);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "end_stroke",
            handle: |engine, _payload, _b| {
                engine.end_stroke();
                Ok(Response::empty())
            },
        },
    ]
}
