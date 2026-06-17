//! View transform, canvas resize / rescale / crop, and canvas dimension queries.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "set_view_transform",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    pan_x: f32,
                    pan_y: f32,
                    zoom: f32,
                    rotation: f32,
                    mirror_h: bool,
                    screen_w: f32,
                    screen_h: f32,
                }
                let r: Req = decode(payload)?;
                engine.set_view_transform(
                    r.pan_x, r.pan_y, r.zoom, r.rotation, r.mirror_h, r.screen_w, r.screen_h,
                );
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "resize",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    width: u32,
                    height: u32,
                }
                let r: Req = decode(payload)?;
                engine.resize(r.width, r.height);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "resize_canvas_rect",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    origin_x: i32,
                    origin_y: i32,
                    w: u32,
                    h: u32,
                }
                let r: Req = decode(payload)?;
                let rect = crate::coord::CanvasRect::new(
                    crate::coord::CanvasPoint::new(r.origin_x, r.origin_y),
                    r.w,
                    r.h,
                );
                engine.resize_canvas(rect);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "crop_to_selection",
            handle: |engine, _payload, _b| {
                engine.crop_to_selection();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "rescale_image",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    w: u32,
                    h: u32,
                }
                let r: Req = decode(payload)?;
                engine.rescale_image(r.w, r.h);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "canvas_dimensions",
            handle: |engine, _payload, _b| {
                let (width, height) = engine.canvas_dimensions();
                Ok(Response::json(serde_json::json!({
                    "width": width,
                    "height": height,
                })))
            },
        },
        RequestRegistration {
            kind: "canvas_rect",
            handle: |engine, _payload, _b| {
                let r = engine.canvas_rect();
                Ok(Response::json(serde_json::json!({
                    "origin_x": r.origin.x,
                    "origin_y": r.origin.y,
                    "width": r.width,
                    "height": r.height,
                })))
            },
        },
    ]
}
