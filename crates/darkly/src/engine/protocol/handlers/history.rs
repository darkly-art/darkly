//! Undo / redo requests.

use crate::engine::protocol::{RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "undo",
            handle: |engine, _payload, _b| {
                engine.undo();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "redo",
            handle: |engine, _payload, _b| {
                engine.redo();
                Ok(Response::empty())
            },
        },
    ]
}
