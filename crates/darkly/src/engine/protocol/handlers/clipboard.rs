//! Copy / cut / paste, including rich (metadata-bearing) layer clipboard.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::protocol::{bad_payload, decode, layer_id, RequestRegistration, Response};
use crate::layer::LayerId;

/// `active_layer_id` follows the f64 negative-means-none FFI convention.
fn active(id: i64) -> Option<LayerId> {
    (id >= 0).then(|| LayerId::from_ffi(id as u64))
}

fn export_value(
    export: Option<crate::engine::ClipboardExport>,
) -> Result<Value, crate::engine::protocol::ProtocolError> {
    match export {
        Some(e) => serde_json::to_value(&e).map_err(bad_payload),
        None => Ok(Value::Null),
    }
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "copy",
            handle: |engine, payload, _b| {
                let export = engine.copy(layer_id(payload)?);
                Ok(Response::json(export_value(export)?))
            },
        },
        RequestRegistration {
            kind: "cut",
            handle: |engine, payload, _b| {
                let export = engine.cut(layer_id(payload)?);
                Ok(Response::json(export_value(export)?))
            },
        },
        RequestRegistration {
            kind: "poll_copy_result",
            handle: |engine, _payload, _b| {
                let export = engine.poll_copy_result();
                Ok(Response::json(export_value(export)?))
            },
        },
        RequestRegistration {
            kind: "copy_layer_rich",
            handle: |engine, payload, _b| {
                engine.copy_layer_rich(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "poll_copy_rich_result",
            handle: |engine, _payload, _b| {
                let value = match engine.poll_copy_rich_result() {
                    Some(s) => Value::String(s),
                    None => Value::Null,
                };
                Ok(Response::json(value))
            },
        },
        RequestRegistration {
            kind: "paste_layer_rich",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    json: String,
                    active_layer_id: i64,
                }
                let r: Req = decode(payload)?;
                let id = match engine.paste_layer_rich(&r.json, active(r.active_layer_id)) {
                    Some(id) => id.to_ffi() as i64,
                    None => -1,
                };
                Ok(Response::json(json!({ "id": id })))
            },
        },
        RequestRegistration {
            kind: "paste_image",
            handle: |engine, payload, bytes| {
                let r: PasteImage = decode(payload)?;
                let id = engine.paste_image(
                    r.width,
                    r.height,
                    bytes,
                    r.offset_x,
                    r.offset_y,
                    active(r.active_layer_id),
                );
                Ok(Response::json(json!({ "id": id.to_ffi() })))
            },
        },
        RequestRegistration {
            kind: "paste_image_floating",
            handle: |engine, payload, bytes| {
                let r: PasteImage = decode(payload)?;
                let id = engine.paste_image_floating(
                    r.width,
                    r.height,
                    bytes,
                    r.offset_x,
                    r.offset_y,
                    active(r.active_layer_id),
                );
                Ok(Response::json(json!({ "id": id.to_ffi() })))
            },
        },
        RequestRegistration {
            kind: "paste_in_place",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    active_layer_id: i64,
                }
                let r: Req = decode(payload)?;
                let id = match engine.paste_in_place(active(r.active_layer_id)) {
                    Some(id) => id.to_ffi() as i64,
                    None => -1,
                };
                Ok(Response::json(json!({ "id": id })))
            },
        },
    ]
}

#[derive(Deserialize)]
struct PasteImage {
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
    active_layer_id: i64,
}
