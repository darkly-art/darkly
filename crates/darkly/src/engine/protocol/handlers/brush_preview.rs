//! Brush / node preview rendering — node thumbnails, stroke / dab previews, and
//! the live cursor-preview pose. All image responses are PNG bytes on the binary
//! side-channel; the cursor-preview info is a small JSON envelope.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;
use serde_json::Value;

/// Encode the active cursor-preview info (or `null` when absent).
fn preview_info(engine: &crate::engine::DarklyEngine) -> serde_json::Value {
    match engine.brush_cursor_preview_info() {
        Some(info) => serde_json::json!({ "halfExtent": info.half_extent_canvas_px }),
        None => serde_json::Value::Null,
    }
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("node_thumbnail", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                node_id: u64,
                width: u32,
                height: u32,
            }
            let r: Req = decode(payload)?;
            let bytes = engine.node_thumbnail(LayerId::from_ffi(r.node_id), r.width, r.height);
            Ok(Response::binary(serde_json::Value::Null, bytes))
        }),
        RequestRegistration::new::<Value, Value>("brush_stroke_preview", |engine, _payload, _b| {
            Ok(Response::binary(
                serde_json::Value::Null,
                engine.brush_stroke_preview(),
            ))
        }),
        RequestRegistration::new::<Value, Value>(
            "brush_active_dab_preview",
            |engine, _payload, _b| {
                Ok(Response::binary(
                    serde_json::Value::Null,
                    engine.brush_active_dab_preview(),
                ))
            },
        ),
        RequestRegistration::new::<Value, Value>("brush_node_preview", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                node_id: u64,
            }
            let r: Req = decode(payload)?;
            Ok(Response::binary(
                serde_json::Value::Null,
                engine.brush_node_preview(r.node_id),
            ))
        }),
        RequestRegistration::new::<Value, Value>("brush_thumbnail", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                name: String,
            }
            let r: Req = decode(payload)?;
            Ok(Response::binary(
                serde_json::Value::Null,
                engine.brush_thumbnail(&r.name),
            ))
        }),
        RequestRegistration::new::<Value, Value>("brush_dab_thumbnail", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                name: String,
            }
            let r: Req = decode(payload)?;
            Ok(Response::binary(
                serde_json::Value::Null,
                engine.brush_dab_thumbnail(&r.name),
            ))
        }),
        RequestRegistration::new::<Value, Value>(
            "clear_brush_cursor_preview_pose",
            |engine, _payload, _b| {
                engine.clear_brush_cursor_preview_pose();
                Ok(Response::empty())
            },
        ),
        RequestRegistration::new::<Value, Value>(
            "get_brush_cursor_preview_info",
            |engine, _payload, _b| Ok(Response::json(preview_info(engine))),
        ),
        RequestRegistration::new::<Value, Value>(
            "refresh_brush_cursor_preview",
            |engine, payload, _b| {
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
        ),
    ]
}
