//! Document-level metadata — name and dirty state.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("document_name", |engine, _payload, _b| {
            Ok(Response::json(serde_json::json!({
                "name": engine.document_name(),
            })))
        }),
        RequestRegistration::new::<Value, Value>("set_document_name", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                name: String,
            }
            let r: Req = decode(payload)?;
            engine.set_document_name(r.name);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("is_dirty", |engine, _payload, _b| {
            Ok(Response::json(serde_json::json!({
                "value": engine.is_dirty(),
            })))
        }),
        RequestRegistration::new::<Value, Value>("mark_dirty", |engine, _payload, _b| {
            engine.mark_dirty();
            Ok(Response::empty())
        }),
    ]
}
