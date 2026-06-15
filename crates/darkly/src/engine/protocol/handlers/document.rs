//! Document-level metadata — name and dirty state.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "document_name",
            handle: |engine, _payload, _b| {
                Ok(Response::json(serde_json::json!({
                    "name": engine.document_name(),
                })))
            },
        },
        RequestRegistration {
            kind: "set_document_name",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    name: String,
                }
                let r: Req = decode(payload)?;
                engine.set_document_name(r.name);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "is_dirty",
            handle: |engine, _payload, _b| {
                Ok(Response::json(serde_json::json!({
                    "value": engine.is_dirty(),
                })))
            },
        },
    ]
}
