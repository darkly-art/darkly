//! Text / vector-layer protocol handlers — create a text layer, hit-test and
//! edit its objects (content / style / transform), and list available fonts.
//! Auto-discovered by `build.rs`.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::{LayerId, ObjectId, TextAlign, TextProps, TextStyle};

fn parse_align(s: &str) -> TextAlign {
    match s {
        "center" => TextAlign::Center,
        "end" => TextAlign::End,
        "justified" => TextAlign::Justified,
        _ => TextAlign::Start,
    }
}

fn align_str(a: TextAlign) -> &'static str {
    match a {
        TextAlign::Start => "start",
        TextAlign::Center => "center",
        TextAlign::End => "end",
        TextAlign::Justified => "justified",
    }
}

fn parse_style(italic: bool) -> TextStyle {
    if italic {
        TextStyle::Italic
    } else {
        TextStyle::Normal
    }
}

/// Build a [`TextProps`] from the wire fields shared by `add_text` (new layer)
/// and `add_text_object` (existing layer). `None` style fields take the
/// [`TextProps::new`] defaults; `box` makes it area text.
#[allow(clippy::too_many_arguments)]
fn build_text_props(
    content: String,
    font_family: Option<String>,
    size: Option<f32>,
    weight: Option<f32>,
    italic: bool,
    align: Option<String>,
    box_size: Option<[f32; 2]>,
) -> TextProps {
    let mut text = TextProps::new(content);
    if let Some(f) = font_family {
        text.font_family = f;
    }
    if let Some(s) = size {
        text.size = s;
    }
    if let Some(w) = weight {
        text.weight = w;
    }
    text.style = parse_style(italic);
    if let Some(a) = align {
        text.align = parse_align(&a);
    }
    text.box_size = box_size.map(|b| (b[0], b[1]));
    text
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
                    /// `[w, h]` for a drag-created area-text box; absent → point text.
                    #[serde(default)]
                    r#box: Option<[f32; 2]>,
                    anchor: i64,
                }
                let r: Req = decode(payload)?;
                let text = build_text_props(
                    r.content,
                    r.font_family,
                    r.size,
                    r.weight,
                    r.italic,
                    r.align,
                    r.r#box,
                );
                let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
                let color = r.color.unwrap_or([0, 0, 0, 255]);
                let (id, object) = engine.add_text_layer(text, r.x, r.y, color, anchor);
                // Return the seeded object id alongside the layer id so the panel
                // can address the new text object without a follow-up query.
                Ok(Response::json(
                    json!({ "id": id.to_ffi(), "object": object.0 }),
                ))
            },
        },
        RequestRegistration {
            kind: "add_text_object",
            handle: |engine, payload, _b| {
                // Add another text object to an existing vector layer — the
                // multi-object case (placing text while a text layer is active).
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
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
                    /// `[w, h]` for a drag-created area-text box; absent → point text.
                    #[serde(default)]
                    r#box: Option<[f32; 2]>,
                }
                let r: Req = decode(payload)?;
                let text = build_text_props(
                    r.content,
                    r.font_family,
                    r.size,
                    r.weight,
                    r.italic,
                    r.align,
                    r.r#box,
                );
                let color = r.color.unwrap_or([0, 0, 0, 255]);
                // `-1` for a non-vector id, mirroring `hit_test_vector_object`.
                let object = engine
                    .add_text_object(LayerId::from_ffi(r.id), text, r.x, r.y, color)
                    .map(|o| o.0 as i64)
                    .unwrap_or(-1);
                Ok(Response::json(json!({ "object": object })))
            },
        },
        RequestRegistration {
            kind: "set_text_content",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    object: u64,
                    content: String,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                let object = ObjectId(r.object);
                engine.set_text_content(id, object, r.content);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "set_text_style",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    object: u64,
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
                let object = ObjectId(r.object);
                engine.set_text_style(
                    id,
                    object,
                    r.font_family,
                    r.size,
                    r.weight,
                    r.italic,
                    r.align.as_deref().map(parse_align),
                    r.color,
                );
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "hit_test_vector_object",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    x: f64,
                    y: f64,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                // `-1` sentinel for a miss, mirroring the `anchor: i64` convention.
                let object = engine
                    .hit_test_vector_object(id, r.x, r.y)
                    .map(|o| o.0 as i64)
                    .unwrap_or(-1);
                Ok(Response::json(json!({ "object": object })))
            },
        },
        RequestRegistration {
            kind: "text_objects",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                let objects: Vec<_> = engine
                    .text_objects(id)
                    .into_iter()
                    .map(|o| {
                        json!({
                            "object": o.object.0,
                            "content": o.content,
                            "font_family": o.font_family,
                            "size": o.size,
                            "weight": o.weight,
                            "italic": o.italic,
                            "align": align_str(o.align),
                            "color": o.color,
                            "box": o.box_size.map(|(w, h)| [w, h]),
                        })
                    })
                    .collect();
                Ok(Response::json(json!({ "objects": objects })))
            },
        },
        RequestRegistration {
            kind: "vector_object_info",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    object: u64,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                let value = match engine.vector_object_info(id, ObjectId(r.object)) {
                    Some((ox, oy, w, h, matrix)) => json!({
                        "ox": ox, "oy": oy, "w": w, "h": h,
                        "mode": 0, "matrix": matrix,
                    }),
                    None => serde_json::Value::Null,
                };
                Ok(Response::json(value))
            },
        },
        RequestRegistration {
            kind: "update_vector_object_transform",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    object: u64,
                    payload: Vec<f32>,
                }
                let r: Req = decode(payload)?;
                if r.payload.len() >= 6 {
                    // The gizmo ships the full canvas affine row-major; the
                    // engine reorders to kurbo and strips the layer transform.
                    let g = crate::transform::Transform::from_affine([
                        r.payload[0],
                        r.payload[1],
                        r.payload[2],
                        r.payload[3],
                        r.payload[4],
                        r.payload[5],
                    ]);
                    let id = LayerId::from_ffi(r.id);
                    engine.set_vector_object_transform(id, ObjectId(r.object), g);
                }
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "set_text_box",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    object: u64,
                    /// Full canvas affine `G` (row-major) for the box's moved
                    /// origin; the engine strips the layer transform.
                    matrix: [f32; 6],
                    /// Box size `[w, h]` in canvas pixels.
                    r#box: [f32; 2],
                }
                let r: Req = decode(payload)?;
                let g = crate::transform::Transform::from_affine(r.matrix);
                let id = LayerId::from_ffi(r.id);
                engine.set_text_box(id, ObjectId(r.object), g, (r.r#box[0], r.r#box[1]));
                Ok(Response::empty())
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
