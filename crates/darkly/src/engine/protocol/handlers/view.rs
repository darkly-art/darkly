//! View transform, canvas resize / rescale / crop, and canvas dimension queries.

use serde::{Deserialize, Serialize};

use crate::engine::protocol::{decode, RequestRegistration, Response};

/// `{ origin_x, origin_y, w, h }` — a new canvas window rect (flat ints, not the
/// nested `CanvasRect` serde shape).
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct ResizeCanvasRectReq {
    pub origin_x: i32,
    pub origin_y: i32,
    pub w: u32,
    pub h: u32,
}

/// Horizontal / vertical flip axis.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum FlipAxis {
    H,
    V,
}

/// `{ axis }` — which way to flip the canvas.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct FlipCanvasReq {
    pub axis: FlipAxis,
}

/// Rotation direction (`180` is a half turn).
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum RotateDir {
    Cw,
    Ccw,
    #[serde(rename = "180")]
    Half,
}

/// `{ dir }` — which way to rotate the canvas.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct RotateCanvasReq {
    pub dir: RotateDir,
}

/// Canvas pixel dimensions.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct CanvasDimensionsResp {
    pub width: u32,
    pub height: u32,
}

/// The canvas window rect, flattened (`origin_x`/`origin_y` + size).
#[derive(Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct CanvasRectResp {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("resize_canvas_rect", |engine, payload, _b| {
            let r: ResizeCanvasRectReq = decode(payload)?;
            let rect = crate::coord::CanvasRect::new(
                crate::coord::CanvasPoint::new(r.origin_x, r.origin_y),
                r.w,
                r.h,
            );
            engine.resize_canvas(rect);
            Ok(Response::empty())
        })
        .post()
        .req::<ResizeCanvasRectReq>(),
        RequestRegistration::new("flip_canvas", |engine, payload, _b| {
            let r: FlipCanvasReq = decode(payload)?;
            engine.transform_canvas(match r.axis {
                FlipAxis::H => crate::gpu::ortho_transform::OrthoXform::FlipH,
                FlipAxis::V => crate::gpu::ortho_transform::OrthoXform::FlipV,
            });
            Ok(Response::empty())
        })
        .post()
        .req::<FlipCanvasReq>(),
        RequestRegistration::new("rotate_canvas", |engine, payload, _b| {
            let r: RotateCanvasReq = decode(payload)?;
            engine.transform_canvas(match r.dir {
                RotateDir::Cw => crate::gpu::ortho_transform::OrthoXform::Rot90Cw,
                RotateDir::Ccw => crate::gpu::ortho_transform::OrthoXform::Rot90Ccw,
                RotateDir::Half => crate::gpu::ortho_transform::OrthoXform::Rot180,
            });
            Ok(Response::empty())
        })
        .post()
        .req::<RotateCanvasReq>(),
        RequestRegistration::new("canvas_dimensions", |engine, _payload, _b| {
            let (width, height) = engine.canvas_dimensions();
            Ok(Response::json(
                serde_json::to_value(CanvasDimensionsResp { width, height })
                    .map_err(crate::engine::protocol::bad_payload)?,
            ))
        })
        .send()
        .resp::<CanvasDimensionsResp>(),
        RequestRegistration::new("canvas_rect", |engine, _payload, _b| {
            let r = engine.canvas_rect();
            Ok(Response::json(
                serde_json::to_value(CanvasRectResp {
                    origin_x: r.origin.x,
                    origin_y: r.origin.y,
                    width: r.width,
                    height: r.height,
                })
                .map_err(crate::engine::protocol::bad_payload)?,
            ))
        })
        .send()
        .resp::<CanvasRectResp>(),
    ]
}
