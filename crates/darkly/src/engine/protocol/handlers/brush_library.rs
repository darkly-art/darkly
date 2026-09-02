//! Brush-pack export. The rest of the brush library (list / save / load /
//! import / packs / thumbnails) is `#[handler]`-generated on
//! `engine/brush_library.rs`; `pack_export` stays hand-written because it's a
//! *fallible* binary response (`Result<Vec<u8>, String>`): the
//! `returns = bytes` mode is infallible, and the engine error must reject
//! rather than ride the side-channel.

use serde::Deserialize;

use crate::engine::protocol::{decode, ProtocolError, RequestRegistration, Response};

/// `{ id }`: the pack to export as a `.darkly-brush` archive.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct PackExportReq {
    pub id: String,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("pack_export", |engine, payload, _b| {
            let r: PackExportReq = decode(payload)?;
            let bytes = engine.pack_export(&r.id).map_err(ProtocolError::engine)?;
            Ok(Response::binary(serde_json::Value::Null, bytes))
        })
        .send()
        .req::<PackExportReq>()
        .resp_literal("{ bytes: Uint8Array }"),
    ]
}
