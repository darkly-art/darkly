//! Preview / viewport rendering configuration requests.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "set_preview_theme",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    fg: [f32; 4],
                    bg: [f32; 4],
                }
                let r: Req = decode(payload)?;
                engine.set_preview_theme(r.fg, r.bg);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "set_viewport_bg",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    bg: [f32; 4],
                }
                let r: Req = decode(payload)?;
                engine.set_viewport_bg(r.bg);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "set_pixel_filter",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    mode: String,
                }
                let r: Req = decode(payload)?;
                engine.set_pixel_filter(&r.mode);
                Ok(Response::empty())
            },
        },
    ]
}
