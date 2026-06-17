//! Selection requests — rect / ellipse / lasso / magic-wand, and selection state.

use serde::Deserialize;

use crate::engine::protocol::{decode, layer_id, RequestRegistration, Response};

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
        RequestRegistration {
            kind: "select_rect",
            handle: |engine, payload, _b| {
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
            },
        },
        RequestRegistration {
            kind: "select_ellipse",
            handle: |engine, payload, _b| {
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
            },
        },
        RequestRegistration {
            kind: "select_lasso",
            handle: |engine, payload, _b| {
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
            },
        },
        RequestRegistration {
            kind: "select_magic_wand",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    layer_id: u64,
                    seed_x: i32,
                    seed_y: i32,
                    tolerance: u8,
                    mode: String,
                }
                let r: Req = decode(payload)?;
                engine.select_magic_wand(
                    crate::layer::LayerId::from_ffi(r.layer_id),
                    crate::coord::CanvasPoint::new(r.seed_x, r.seed_y),
                    r.tolerance,
                    parse_mode(&r.mode),
                );
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "clear_selection",
            handle: |engine, _payload, _b| {
                engine.clear_selection();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "clear_selection_contents",
            handle: |engine, payload, _b| {
                engine.clear_selection_contents(layer_id(payload)?);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "select_all",
            handle: |engine, _payload, _b| {
                engine.select_all();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "invert_selection",
            handle: |engine, _payload, _b| {
                engine.invert_selection();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "has_selection",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::json!({ "value": engine.has_selection() }),
                ))
            },
        },
    ]
}
