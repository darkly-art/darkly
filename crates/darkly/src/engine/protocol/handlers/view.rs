//! View transform, canvas resize / rescale / crop, and canvas dimension queries.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("set_view_transform", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("resize", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                width: u32,
                height: u32,
            }
            let r: Req = decode(payload)?;
            engine.resize(r.width, r.height);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("resize_canvas_rect", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("crop_to_selection", |engine, _payload, _b| {
            engine.crop_to_selection();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("rescale_image", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                w: u32,
                h: u32,
            }
            let r: Req = decode(payload)?;
            engine.rescale_image(r.w, r.h);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("flip_canvas", |engine, payload, _b| {
            #[derive(Deserialize)]
            #[serde(rename_all = "lowercase")]
            enum Axis {
                H,
                V,
            }
            #[derive(Deserialize)]
            struct Req {
                axis: Axis,
            }
            let r: Req = decode(payload)?;
            engine.transform_canvas(match r.axis {
                Axis::H => crate::gpu::ortho_transform::OrthoXform::FlipH,
                Axis::V => crate::gpu::ortho_transform::OrthoXform::FlipV,
            });
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("rotate_canvas", |engine, payload, _b| {
            #[derive(Deserialize)]
            #[serde(rename_all = "lowercase")]
            enum Dir {
                Cw,
                Ccw,
                #[serde(rename = "180")]
                Half,
            }
            #[derive(Deserialize)]
            struct Req {
                dir: Dir,
            }
            let r: Req = decode(payload)?;
            engine.transform_canvas(match r.dir {
                Dir::Cw => crate::gpu::ortho_transform::OrthoXform::Rot90Cw,
                Dir::Ccw => crate::gpu::ortho_transform::OrthoXform::Rot90Ccw,
                Dir::Half => crate::gpu::ortho_transform::OrthoXform::Rot180,
            });
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("canvas_dimensions", |engine, _payload, _b| {
            let (width, height) = engine.canvas_dimensions();
            Ok(Response::json(serde_json::json!({
                "width": width,
                "height": height,
            })))
        }),
        RequestRegistration::new::<Value, Value>("canvas_rect", |engine, _payload, _b| {
            let r = engine.canvas_rect();
            Ok(Response::json(serde_json::json!({
                "origin_x": r.origin.x,
                "origin_y": r.origin.y,
                "width": r.width,
                "height": r.height,
            })))
        }),
    ]
}
