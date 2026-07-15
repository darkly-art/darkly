//! Brush-bundle export. The rest of the brush library (list / save / load /
//! import / thumbnails) is `#[handler]`-generated on `engine/brush_library.rs`;
//! `brush_export` stays hand-written because it's a *fallible* binary response
//! (`Result<Vec<u8>, String>`) — the `returns = bytes` mode is infallible, and
//! the engine error must reject rather than ride the side-channel.

use serde::Deserialize;

use crate::engine::protocol::{decode, ProtocolError, RequestRegistration, Response};

/// `{ name }` — the brush to export as a bundle.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushExportReq {
    pub name: String,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("brush_export", |engine, payload, _b| {
            let r: BrushExportReq = decode(payload)?;
            let bytes = engine
                .brush_export(&r.name)
                .map_err(ProtocolError::engine)?;
            Ok(Response::binary(serde_json::Value::Null, bytes))
        })
        .send()
        .req::<BrushExportReq>()
        .resp_literal("{ bytes: Uint8Array }"),
    ]
}
