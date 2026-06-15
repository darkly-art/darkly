//! Image export (PNG/JPEG/WebP readback) and native `.darkly` save / open.
//!
//! Multi-blob outputs (`poll_save_result`) concatenate every byte buffer into
//! the single [`Response`] `bytes` side-channel; the JSON value carries the
//! lengths so the JS edge can slice them back out in order.

use serde_json::json;

use crate::engine::protocol::{ProtocolError, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "start_export",
            handle: |engine, _payload, _b| {
                engine.start_export();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "poll_export_result",
            handle: |engine, _payload, _b| {
                let Some(result) = engine.poll_export_result() else {
                    return Ok(Response::json(serde_json::Value::Null));
                };
                let value = json!({ "width": result.width, "height": result.height });
                Ok(Response::binary(value, result.rgba))
            },
        },
        RequestRegistration {
            kind: "start_save_document",
            handle: |engine, _payload, _b| match engine.start_save_document() {
                Ok(()) => Ok(Response::empty()),
                Err(e) => Err(ProtocolError::engine(e.to_string())),
            },
        },
        RequestRegistration {
            kind: "poll_save_result",
            handle: |engine, _payload, _b| {
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
            },
        },
        RequestRegistration {
            kind: "open_document",
            handle: |engine, _payload, bytes| match engine.open_document(bytes) {
                Ok(()) => Ok(Response::empty()),
                // The structured LoadError JSON rides in the rejection message;
                // the JS open caller `JSON.parse`s it for the LoadErrorToast.
                Err(e) => Err(ProtocolError::engine(e.to_json().to_string())),
            },
        },
    ]
}
