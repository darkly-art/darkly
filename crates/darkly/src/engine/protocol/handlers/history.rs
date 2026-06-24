//! Undo / redo requests.

use crate::engine::protocol::{RequestRegistration, Response};
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("undo", |engine, _payload, _b| {
            engine.undo();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("redo", |engine, _payload, _b| {
            engine.redo();
            Ok(Response::empty())
        }),
    ]
}
