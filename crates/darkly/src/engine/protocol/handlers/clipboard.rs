//! Paste (rich layer / image / in-place). The copy/cut side and the poll
//! queries are `#[handler]`-generated on `engine/clipboard.rs`; the pastes stay
//! hand-written because they share the `active_layer_id` f64-sentinel
//! (`-1`→none) → `Option<LayerId>` coercion and re-encode their `Option<LayerId>`
//! result as `{ id: -1 }`, neither of which the macro derives.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;

/// `active_layer_id` follows the f64 negative-means-none FFI convention.
fn active(id: i64) -> Option<LayerId> {
    (id >= 0).then(|| LayerId::from_ffi(id as u64))
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
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
