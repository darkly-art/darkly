//! Live cursor-preview pose. The thumbnail / stroke / dab byte responses and
//! the pose-clear verb are `#[handler]`-generated (`returns = bytes` on the
//! engine methods); these two stay hand-written because their JSON response is
//! a derived `{ halfExtent }` envelope shaped from `BrushCursorPreviewInfo`, and
//! `refresh` marshals a flat pen pose into a `PaintInformation` before rendering.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};

/// Encode the active cursor-preview info (or `null` when absent).
fn preview_info(engine: &crate::engine::DarklyEngine) -> serde_json::Value {
    match engine.brush_cursor_preview_info() {
        Some(info) => serde_json::json!({ "halfExtent": info.half_extent_canvas_px }),
        None => serde_json::Value::Null,
    }
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "get_brush_cursor_preview_info",
            handle: |engine, _payload, _b| Ok(Response::json(preview_info(engine))),
        },
        RequestRegistration {
            kind: "refresh_brush_cursor_preview",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    x: f32,
                    y: f32,
                    pressure: f32,
                    tilt_x: f32,
                    tilt_y: f32,
                    rotation: f32,
                    tangential_pressure: f32,
                }
                let r: Req = decode(payload)?;
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
                Ok(Response::json(preview_info(engine)))
            },
        },
    ]
}
