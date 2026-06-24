//! Brush library — list, export, save, load, and import of brush bundles.

use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

use crate::engine::protocol::{bad_payload, decode, ProtocolError, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("brush_list", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.brush_list()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("brush_export", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                name: String,
            }
            let r: Req = decode(payload)?;
            let bytes = engine
                .brush_export(&r.name)
                .map_err(ProtocolError::engine)?;
            Ok(Response::binary(serde_json::Value::Null, bytes))
        }),
        RequestRegistration::new::<Value, Value>("brush_save", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("brush_load", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                name: String,
            }
            let r: Req = decode(payload)?;
            engine.brush_load(&r.name).map_err(ProtocolError::engine)?;
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("brush_import", |engine, _payload, bytes| {
            let name = engine.brush_import(bytes).map_err(ProtocolError::engine)?;
            Ok(Response::json(json!({ "name": name })))
        }),
    ]
}
