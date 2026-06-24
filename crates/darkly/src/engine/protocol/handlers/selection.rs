//! Selection requests — rect / ellipse / lasso / magic-wand, and selection state.

use serde::Deserialize;

use crate::engine::protocol::{decode, layer_id, RequestRegistration, Response};
use serde_json::Value;

fn parse_mode(s: &str) -> crate::document::SelectionMode {
    use crate::document::SelectionMode::*;
    match s {
        "add" => Add,
        "subtract" => Subtract,
        "intersect" => Intersect,
        _ => Replace,
    }
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("select_rect", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                x: f32,
                y: f32,
                w: f32,
                h: f32,
                mode: String,
                antialias: bool,
                feather: f32,
            }
            let r: Req = decode(payload)?;
            engine.select_rect(
                r.x,
                r.y,
                r.w,
                r.h,
                parse_mode(&r.mode),
                r.antialias,
                r.feather,
            );
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("select_ellipse", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                x: f32,
                y: f32,
                w: f32,
                h: f32,
                mode: String,
                antialias: bool,
                feather: f32,
            }
            let r: Req = decode(payload)?;
            engine.select_ellipse(
                r.x,
                r.y,
                r.w,
                r.h,
                parse_mode(&r.mode),
                r.antialias,
                r.feather,
            );
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("select_lasso", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                verts: Vec<[f32; 2]>,
                mode: String,
                antialias: bool,
                feather: f32,
            }
            let r: Req = decode(payload)?;
            engine.select_lasso(&r.verts, parse_mode(&r.mode), r.antialias, r.feather);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("select_magic_wand", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                seed_x: i32,
                seed_y: i32,
                tolerance: u8,
                mode: String,
            }
            let r: Req = decode(payload)?;
            engine.select_magic_wand(
                crate::layer::LayerId::from_ffi(r.id),
                crate::coord::CanvasPoint::new(r.seed_x, r.seed_y),
                r.tolerance,
                parse_mode(&r.mode),
            );
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("clear_selection", |engine, _payload, _b| {
            engine.clear_selection();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>(
            "clear_selection_contents",
            |engine, payload, _b| {
                engine.clear_selection_contents(layer_id(payload)?);
                Ok(Response::empty())
            },
        ),
        RequestRegistration::new::<Value, Value>("select_all", |engine, _payload, _b| {
            engine.select_all();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("invert_selection", |engine, _payload, _b| {
            engine.invert_selection();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("grow_selection", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                radius: u32,
            }
            let r: Req = decode(payload)?;
            engine.grow_selection(r.radius);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("shrink_selection", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                radius: u32,
            }
            let r: Req = decode(payload)?;
            engine.shrink_selection(r.radius);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("border_selection", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                radius: u32,
            }
            let r: Req = decode(payload)?;
            engine.border_selection(r.radius);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("smooth_selection", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                radius: u32,
            }
            let r: Req = decode(payload)?;
            engine.smooth_selection(r.radius);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("feather_selection", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                radius: f32,
            }
            let r: Req = decode(payload)?;
            engine.feather_selection(r.radius);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("antialias_selection", |engine, _payload, _b| {
            engine.antialias_selection();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("has_selection", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::json!({ "value": engine.has_selection() }),
            ))
        }),
    ]
}
