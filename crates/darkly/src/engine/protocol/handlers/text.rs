//! Text / vector-layer protocol handlers — create a text layer, edit its
//! content/style, and list available fonts. Auto-discovered by `build.rs`.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::{LayerId, TextAlign, TextProps, TextStyle};

fn parse_align(s: &str) -> TextAlign {
    match s {
        "center" => TextAlign::Center,
        "end" => TextAlign::End,
        "justified" => TextAlign::Justified,
        _ => TextAlign::Start,
    }
}

fn parse_style(italic: bool) -> TextStyle {
    if italic {
        TextStyle::Italic
    } else {
        TextStyle::Normal
    }
}

/// `{ width, height }` shaped-bounds envelope returned by the create/edit
/// handlers so the frontend can position its editing overlay.
fn bounds_json(engine: &mut crate::engine::DarklyEngine, id: LayerId) -> serde_json::Value {
    let (w, h) = engine.text_layer_bounds(id).unwrap_or((0.0, 0.0));
    json!({ "id": id.to_ffi(), "width": w, "height": h })
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "add_text",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    content: String,
                    #[serde(default)]
                    font_family: Option<String>,
                    #[serde(default)]
                    size: Option<f32>,
                    #[serde(default)]
                    weight: Option<f32>,
                    #[serde(default)]
                    italic: bool,
                    #[serde(default)]
                    align: Option<String>,
                    x: f64,
                    y: f64,
                    /// RGBA 0–255. Defaults to opaque black.
                    #[serde(default)]
                    color: Option<[u8; 4]>,
                    anchor: i64,
                }
                let r: Req = decode(payload)?;
                let mut text = TextProps::new(r.content);
                if let Some(f) = r.font_family {
                    text.font_family = f;
                }
                if let Some(s) = r.size {
                    text.size = s;
                }
                if let Some(w) = r.weight {
                    text.weight = w;
                }
                text.style = parse_style(r.italic);
                if let Some(a) = r.align {
                    text.align = parse_align(&a);
                }
                let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
                let color = r.color.unwrap_or([0, 0, 0, 255]);
                let id = engine.add_text_layer(text, r.x, r.y, color, anchor);
                Ok(Response::json(bounds_json(engine, id)))
            },
        },
        RequestRegistration {
            kind: "set_text_content",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    content: String,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                engine.set_text_content(id, r.content);
                Ok(Response::json(bounds_json(engine, id)))
            },
        },
        RequestRegistration {
            kind: "set_text_style",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    #[serde(default)]
                    font_family: Option<String>,
                    #[serde(default)]
                    size: Option<f32>,
                    #[serde(default)]
                    weight: Option<f32>,
                    #[serde(default)]
                    italic: Option<bool>,
                    #[serde(default)]
                    align: Option<String>,
                    #[serde(default)]
                    color: Option<[u8; 4]>,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                engine.set_text_style(
                    id,
                    r.font_family,
                    r.size,
                    r.weight,
                    r.italic,
                    r.align.as_deref().map(parse_align),
                    r.color,
                );
                Ok(Response::json(bounds_json(engine, id)))
            },
        },
        RequestRegistration {
            kind: "list_fonts",
            handle: |engine, _payload, _b| {
                Ok(Response::json(json!({ "fonts": engine.list_fonts() })))
            },
        },
    ]
}
