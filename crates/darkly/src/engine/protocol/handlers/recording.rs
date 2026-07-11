//! Process recording (timelapse) — capture parameters + frame draining.
//!
//! The engine only captures frames; encoding and persistence are
//! frontend-owned. `poll_recording_frame` mirrors `poll_export_result`:
//! JSON envelope + raw RGBA on the binary side-channel.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};

/// `{ enabled, minIntervalSecs, width, height }` — frontend-negotiated
/// capture parameters. `width`/`height` are the encoder frame dimensions
/// (even-aligned); the engine letterboxes the canvas into them.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetRecordingParamsReq {
    pub enabled: bool,
    pub min_interval_secs: f32,
    pub width: u32,
    pub height: u32,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("set_recording_params", |engine, payload, _b| {
            let r: SetRecordingParamsReq = decode(payload)?;
            engine.set_recording_params(r.enabled, r.min_interval_secs, r.width, r.height);
            Ok(Response::empty())
        })
        .post()
        .req::<SetRecordingParamsReq>(),
        RequestRegistration::new("poll_recording_frame", |engine, _payload, _b| {
            let Some(frame) = engine.poll_recording_frame() else {
                return Ok(Response::json(serde_json::Value::Null));
            };
            let value = json!({
                "width": frame.width,
                "height": frame.height,
                "frameIndex": frame.frame_index,
            });
            Ok(Response::binary(value, frame.rgba))
        })
        .send()
        .resp_literal(
            "{ width: number, height: number, frameIndex: number, bytes: Uint8Array } | null",
        ),
    ]
}
