//! Picker previews (veil + void): one generic start/poll request pair.
//!
//! Both effect kinds render small looping thumbnails read back asynchronously;
//! once [`PreviewKind`] exists the start/poll handlers are identical except for
//! which engine entry point `start` dispatches to, so a single pair (keyed on a
//! `kind` field) serves both pickers rather than two byte-identical copies.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::engine::PreviewKind;

#[derive(Deserialize)]
struct PreviewReq {
    kind: String,
    #[serde(rename = "type")]
    type_id: String,
}

/// Resolve the wire `kind` string to a [`PreviewKind`]. Unknown kinds return
/// `None` so the handler can no-op rather than panic on malformed input.
fn parse_kind(kind: &str) -> Option<PreviewKind> {
    match kind {
        "veil" => Some(PreviewKind::Veil),
        "void" => Some(PreviewKind::Void),
        _ => None,
    }
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "start_preview",
            handle: |engine, payload, _b| {
                let r: PreviewReq = decode(payload)?;
                match parse_kind(&r.kind) {
                    Some(PreviewKind::Veil) => engine.start_veil_preview(&r.type_id),
                    Some(PreviewKind::Void) => engine.start_void_preview(&r.type_id),
                    None => {}
                }
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "poll_preview",
            handle: |engine, payload, _b| {
                let r: PreviewReq = decode(payload)?;
                let Some(kind) = parse_kind(&r.kind) else {
                    return Ok(Response::json(serde_json::Value::Null));
                };
                let Some((width, height, frames)) = engine.poll_preview(kind, &r.type_id) else {
                    return Ok(Response::json(serde_json::Value::Null));
                };
                // Frames are concatenated into the single bytes side-channel;
                // the JS edge slices them back out using width*height*4 stride.
                let fps = crate::gpu::preview::PREVIEW_FPS;
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
            },
        },
    ]
}
