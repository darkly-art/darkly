//! Live cursor-preview pose. The thumbnail / stroke / dab byte responses and
//! the pose-clear verb are `#[handler]`-generated (`returns = bytes` on the
//! engine methods); these two stay hand-written because their JSON response is
//! a derived `{ halfExtent }` envelope shaped from `BrushCursorPreviewInfo`, and
//! `refresh` marshals a flat pen pose into a `PaintInformation` before rendering.

use serde::{Deserialize, Serialize};

use crate::engine::protocol::{bad_payload, decode, RequestRegistration, Response};

/// The active cursor preview's half-extent (canvas px), or `null` when there is
/// no preview. The typed `Resp` the brush tool reads instead of a hand-mirror.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct BrushCursorPreviewInfoResp {
    pub half_extent: [f32; 2],
}

/// Flat pen pose for a cursor-preview refresh.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct RefreshBrushCursorPreviewReq {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    pub tangential_pressure: f32,
}

/// Encode the active cursor-preview info (or `null` when absent).
fn preview_info(
    engine: &crate::engine::DarklyEngine,
) -> Result<serde_json::Value, crate::engine::protocol::ProtocolError> {
    match engine.brush_cursor_preview_info() {
        Some(info) => serde_json::to_value(BrushCursorPreviewInfoResp {
            half_extent: info.half_extent_canvas_px,
        })
        .map_err(bad_payload),
        None => Ok(serde_json::Value::Null),
    }
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("get_brush_cursor_preview_info", |engine, _payload, _b| {
            Ok(Response::json(preview_info(engine)?))
        })
        .send()
        .resp::<Option<BrushCursorPreviewInfoResp>>(),
        RequestRegistration::new("refresh_brush_cursor_preview", |engine, payload, _b| {
            let r: RefreshBrushCursorPreviewReq = decode(payload)?;
            let mut pen = crate::brush::paint_info::PaintInformation::cursor_preview_dummy();
            pen.pos = [r.x, r.y];
            if r.pressure > 0.0 {
                pen.pressure = r.pressure;
            }
            pen.x_tilt = r.tilt_x;
            pen.y_tilt = r.tilt_y;
            pen.rotation = r.rotation;
            pen.tangential_pressure = r.tangential_pressure;
            engine.regenerate_brush_cursor_preview_with_pen(pen);
            Ok(Response::json(preview_info(engine)?))
        })
        .send()
        .req::<RefreshBrushCursorPreviewReq>()
        .resp::<Option<BrushCursorPreviewInfoResp>>(),
    ]
}
