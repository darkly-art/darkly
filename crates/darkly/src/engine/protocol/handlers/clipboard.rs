//! Paste (rich layer / image / in-place). The copy/cut side and the poll
//! queries are `#[handler]`-generated on `engine/clipboard.rs`; the pastes stay
//! hand-written because they share the `active_layer_id` f64-sentinel
//! (`-1`→none) → `Option<LayerId>` coercion and re-encode their `Option<LayerId>`
//! result as `{ id: -1 }`, neither of which the macro derives.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;

/// `active_layer_id` follows the f64 negative-means-none FFI convention.
fn active(id: i64) -> Option<LayerId> {
    (id >= 0).then(|| LayerId::from_ffi(id as u64))
}

/// The pasted layer's id, or `-1` when nothing was pasted.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct PasteResultResp {
    pub id: i64,
}

/// `{ json, active_layer_id }` — a serialized rich-layer clipboard payload.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct PasteLayerRichReq {
    pub json: String,
    pub active_layer_id: i64,
}

/// `{ width, height, offset_x, offset_y, active_layer_id }` — a raw RGBA paste;
/// the pixels ride the binary side-channel.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct PasteImageReq {
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub active_layer_id: i64,
}

/// `{ active_layer_id }` — paste the clipboard at its original position.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct PasteInPlaceReq {
    pub active_layer_id: i64,
}

fn paste_result(id: Option<LayerId>) -> Response {
    let id = id.map(|id| id.to_ffi() as i64).unwrap_or(-1);
    Response::json(json!({ "id": id }))
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("paste_layer_rich", |engine, payload, _b| {
            let r: PasteLayerRichReq = decode(payload)?;
            Ok(paste_result(
                engine.paste_layer_rich(&r.json, active(r.active_layer_id)),
            ))
        })
        .send()
        .req::<PasteLayerRichReq>()
        .resp::<PasteResultResp>(),
        RequestRegistration::new("paste_image", |engine, payload, bytes| {
            let r: PasteImageReq = decode(payload)?;
            let id = engine.paste_image(
                r.width,
                r.height,
                bytes,
                r.offset_x,
                r.offset_y,
                active(r.active_layer_id),
            );
            Ok(paste_result(Some(id)))
        })
        .send()
        .bytes_in()
        .req::<PasteImageReq>()
        .resp::<PasteResultResp>(),
        RequestRegistration::new("paste_image_floating", |engine, payload, bytes| {
            let r: PasteImageReq = decode(payload)?;
            let id = engine.paste_image_floating(
                r.width,
                r.height,
                bytes,
                r.offset_x,
                r.offset_y,
                active(r.active_layer_id),
            );
            Ok(paste_result(Some(id)))
        })
        .send()
        .bytes_in()
        .req::<PasteImageReq>()
        .resp::<PasteResultResp>(),
        RequestRegistration::new("paste_in_place", |engine, payload, _b| {
            let r: PasteInPlaceReq = decode(payload)?;
            Ok(paste_result(
                engine.paste_in_place(active(r.active_layer_id)),
            ))
        })
        .send()
        .req::<PasteInPlaceReq>()
        .resp::<PasteResultResp>(),
    ]
}
