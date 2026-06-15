//! Brush library — list, export, save, load, and import of brush bundles.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{bad_payload, decode, ProtocolError, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "brush_list",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.brush_list()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_export",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    name: String,
                }
                let r: Req = decode(payload)?;
                let bytes = engine
                    .brush_export(&r.name)
                    .map_err(ProtocolError::engine)?;
                Ok(Response::binary(serde_json::Value::Null, bytes))
            },
        },
        RequestRegistration {
            kind: "brush_save",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    name: String,
                    category: String,
                }
                let r: Req = decode(payload)?;
                engine
                    .brush_save(&r.name, &r.category)
                    .map_err(ProtocolError::engine)?;
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "brush_load",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    name: String,
                }
                let r: Req = decode(payload)?;
                engine.brush_load(&r.name).map_err(ProtocolError::engine)?;
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "brush_import",
            handle: |engine, _payload, bytes| {
                let name = engine.brush_import(bytes).map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "name": name })))
            },
        },
    ]
}
