//! Image export (PNG/JPEG/WebP readback) and native `.darkly` save / open.
//!
//! Multi-blob outputs (`poll_save_result`) concatenate every byte buffer into
//! the single [`Response`] `bytes` side-channel; the JSON value carries the
//! lengths so the JS edge can slice them back out in order.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{bad_payload, ProtocolError, RequestRegistration, Response};
use crate::engine::SavePurpose;

/// `{ snapshot? }` — a `snapshot` save (autosave to OPFS) leaves the dirty flag
/// set; a file save (the default) clears it.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct StartSaveDocumentReq {
    #[serde(default)]
    pub snapshot: bool,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("poll_export_result", |engine, _payload, _b| {
            let Some(result) = engine.poll_export_result() else {
                return Ok(Response::json(serde_json::Value::Null));
            };
            let value = json!({ "width": result.width, "height": result.height });
            Ok(Response::binary(value, result.rgba))
        })
        .send()
        .resp_literal("{ width: number, height: number, bytes: Uint8Array } | null"),
        RequestRegistration::new("start_save_document", |engine, payload, _b| {
            let r: StartSaveDocumentReq = serde_json::from_value(payload).map_err(bad_payload)?;
            let purpose = if r.snapshot {
                SavePurpose::Snapshot
            } else {
                SavePurpose::File
            };
            match engine.start_save_document(purpose) {
                Ok(()) => Ok(Response::empty()),
                Err(e) => Err(ProtocolError::engine(e.to_string())),
            }
        })
        // Empty success value, but the caller awaits the *rejection*, so `Send`.
        .send()
        .req::<StartSaveDocumentReq>(),
        RequestRegistration::new("poll_save_result", |engine, _payload, _b| {
            let Some(bundle) = engine.poll_save_result() else {
                return Ok(Response::json(serde_json::Value::Null));
            };
            // Pack: manifest ++ composite ++ blob0 ++ blob1 ++ ...
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&bundle.manifest_json);
            bytes.extend_from_slice(&bundle.composite_rgba);
            let blobs: Vec<serde_json::Value> = bundle
                .blobs
                .iter()
                .map(|b| {
                    bytes.extend_from_slice(&b.bytes);
                    json!({ "path": b.path, "len": b.bytes.len() })
                })
                .collect();
            let value = json!({
                "manifestLen": bundle.manifest_json.len(),
                "compositeWidth": bundle.composite_width,
                "compositeHeight": bundle.composite_height,
                "compositeLen": bundle.composite_rgba.len(),
                "blobs": blobs,
            });
            Ok(Response::binary(value, bytes))
        })
        .send()
        .resp_literal(
            "{ manifestLen: number, compositeWidth: number, compositeHeight: number, \
             compositeLen: number, blobs: { path: string, len: number }[], bytes: Uint8Array } | null",
        ),
        RequestRegistration::new("open_document", |engine, _payload, bytes| {
            match engine.open_document(bytes) {
                Ok(()) => Ok(Response::empty()),
                // The structured LoadError JSON rides in the rejection message;
                // the JS open caller `JSON.parse`s it for the LoadErrorToast.
                Err(e) => Err(ProtocolError::engine(e.to_json().to_string())),
            }
        })
        // Bytes-in, empty success value, but the caller awaits the rejection.
        .send()
        .bytes_in(),
    ]
}
