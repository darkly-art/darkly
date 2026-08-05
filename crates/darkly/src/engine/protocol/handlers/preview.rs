//! Picker previews: one generic start/poll request pair over every previewable
//! catalog.
//!
//! The `catalog` field carries a catalog id — the same `"veils"` / `"voids"` /
//! `"filters"` vocabulary `catalogs()` publishes and the frontend's pickers
//! already hold. There is no translation table here and nothing to add when a
//! catalog becomes previewable: the engine looks the id up in the generated
//! mechanism table, and an id it does not find is a no-op.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};

/// `{ catalog, type }` — which catalog and which entry's type id.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct PreviewReq {
    pub catalog: String,
    #[serde(rename = "type")]
    pub type_id: String,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("start_preview", |engine, payload, _b| {
            let r: PreviewReq = decode(payload)?;
            engine.start_preview(&r.catalog, &r.type_id);
            Ok(Response::empty())
        })
        .post()
        .req::<PreviewReq>(),
        RequestRegistration::new("poll_preview", |engine, payload, _b| {
            let r: PreviewReq = decode(payload)?;
            let Some((width, height, fps, frames)) = engine.poll_preview(&r.catalog, &r.type_id)
            else {
                return Ok(Response::json(serde_json::Value::Null));
            };
            // Frames are concatenated into the single bytes side-channel;
            // the JS edge slices them back out using width*height*4 stride.
            // `fps` comes from the entry's own `PreviewAnim` — the one
            // authority on how fast its preview plays.
            let frame_count = frames.len();
            let mut bytes = Vec::new();
            for f in &frames {
                bytes.extend_from_slice(f);
            }
            let value = json!({
                "width": width,
                "height": height,
                "fps": fps,
                "frameCount": frame_count,
            });
            Ok(Response::binary(value, bytes))
        })
        .send()
        .req::<PreviewReq>()
        .resp_literal(
            "{ width: number, height: number, fps: number, frameCount: number, bytes: Uint8Array } | null",
        ),
    ]
}
