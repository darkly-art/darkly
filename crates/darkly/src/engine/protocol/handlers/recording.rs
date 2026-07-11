//! Process recording (timelapse) — capture parameters + frame draining.
//!
//! The engine only captures frames; encoding and persistence are
//! frontend-owned. `poll_recording_frame` mirrors `poll_export_result`:
//! JSON envelope + raw RGBA on the binary side-channel.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};

/// `{ enabled, minIntervalSecs, width, height, baseWidth, baseHeight }` —
/// frontend-negotiated capture parameters. `width`/`height` are the encoder
/// frame dimensions (even-aligned); `baseWidth`/`baseHeight` are the canvas
/// dimensions the negotiation was based on — capture holds while the live
/// canvas aspect ratio differs from theirs.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetRecordingParamsReq {
    pub enabled: bool,
    pub min_interval_secs: f32,
    pub width: u32,
    pub height: u32,
    pub base_width: u32,
    pub base_height: u32,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("set_recording_params", |engine, payload, _b| {
            let r: SetRecordingParamsReq = decode(payload)?;
            engine.set_recording_params(
                r.enabled,
                r.min_interval_secs,
                r.width,
                r.height,
                r.base_width,
                r.base_height,
            );
            Ok(Response::empty())
        })
        .post()
        .req::<SetRecordingParamsReq>(),
        // The response always carries the live canvas dimensions so the
        // per-frame poll doubles as the resize signal: the frontend rolls a
        // new segment when the canvas aspect ratio diverges from the one it
        // negotiated against.
        RequestRegistration::new("poll_recording_frame", |engine, _payload, _b| {
            let (cw, ch) = engine.canvas_dimensions();
            let frame = engine.poll_recording_frame();
            let value = json!({
                "canvasWidth": cw,
                "canvasHeight": ch,
                "frame": frame.as_ref().map(|f| json!({
                    "width": f.width,
                    "height": f.height,
                    "frameIndex": f.frame_index,
                })),
            });
            Ok(match frame {
                Some(f) => Response::binary(value, f.rgba),
                None => Response::json(value),
            })
        })
        .send()
        .resp_literal(
            "{ canvasWidth: number, canvasHeight: number, frame: { width: number, height: number, frameIndex: number } | null, bytes?: Uint8Array }",
        ),
    ]
}
