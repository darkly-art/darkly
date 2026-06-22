//! Image export (PNG/JPEG/WebP readback) and native `.darkly` save / open.
//!
//! `start_export` and `start_save_document` defer: each resolves its own
//! request's promise once the async GPU readback(s) land. The save result is a
//! multi-blob binary payload — every byte buffer concatenated into the single
//! [`Response`] `bytes` side-channel with the lengths in the JSON value
//! (packed by `engine::save::pack_save_bundle`), so the JS edge can slice them
//! back out in order.

use serde::Deserialize;

use crate::engine::protocol::{bad_payload, ProtocolError, RequestRegistration, Response};
use crate::engine::SavePurpose;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "start_export",
            handle: |engine, _payload, _b| {
                engine.start_export();
                Ok(Response::deferred())
            },
        },
        RequestRegistration {
            kind: "start_save_document",
            handle: |engine, payload, _b| {
                // A `snapshot` save (autosave to OPFS) must not clear the
                // document's dirty flag; a file save does. Default to a file
                // save when the flag is absent.
                #[derive(Deserialize)]
                struct Req {
                    #[serde(default)]
                    snapshot: bool,
                }
                let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
                let purpose = if r.snapshot {
                    SavePurpose::Snapshot
                } else {
                    SavePurpose::File
                };
                match engine.start_save_document(purpose) {
                    // The bundle resolves this request when every readback lands.
                    Ok(()) => Ok(Response::deferred()),
                    Err(e) => Err(ProtocolError::engine(e.to_string())),
                }
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
