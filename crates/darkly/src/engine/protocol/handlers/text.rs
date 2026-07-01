//! Text / vector-layer protocol handlers — create a text layer, hit-test and
//! edit its objects (content / style / transform), and list available fonts.
//! Auto-discovered by `build.rs`.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::{LayerId, ObjectId, TextAlign, TextLayout, TextProps, TextStyle};

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

/// The style fields shared by `add_text` (new layer) and `add_text_object`
/// (existing layer). Absent fields take the [`TextProps::new`] defaults; `box`
/// makes it area text. One struct so both handlers deserialize the same shape.
#[derive(Deserialize)]
struct StyleWire {
    #[serde(default)]
    font_family: Option<String>,
    #[serde(default)]
    size: Option<f32>,
    /// Variable-font axis values (tag → value), including `wght`.
    #[serde(default)]
    variations: Option<BTreeMap<String, f32>>,
    /// OpenType feature values (tag → value).
    #[serde(default)]
    features: Option<BTreeMap<String, u32>>,
    #[serde(default)]
    letter_spacing: Option<f32>,
    #[serde(default)]
    word_spacing: Option<f32>,
    #[serde(default)]
    line_height: Option<f32>,
    #[serde(default)]
    italic: bool,
    #[serde(default)]
    align: Option<String>,
    /// `[w, h]` for a drag-created area-text box; absent → point text.
    #[serde(default)]
    r#box: Option<[f32; 2]>,
}

/// Build a [`TextProps`] from the shared wire style fields.
fn build_text_props(content: String, s: StyleWire) -> TextProps {
    let mut text = TextProps::new(content);
    if let Some(f) = s.font_family {
        text.font_family = f;
    }
    if let Some(size) = s.size {
        text.size = size;
    }
    if let Some(vars) = s.variations {
        text.variations = vars;
    }
    if let Some(feats) = s.features {
        text.features = feats;
    }
    if let Some(ls) = s.letter_spacing {
        text.letter_spacing = ls;
    }
    if let Some(ws) = s.word_spacing {
        text.word_spacing = ws;
    }
    if let Some(lh) = s.line_height {
        text.line_height = lh;
    }
    text.style = parse_style(s.italic);
    if let Some(a) = s.align {
        text.align = parse_align(&a);
    }
    if let Some(b) = s.r#box {
        text.layout = TextLayout::Area {
            width: b[0],
            height: b[1],
        };
    }
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
                    #[serde(flatten)]
                    style: StyleWire,
                    x: f64,
                    y: f64,
                    /// RGBA 0–255. Defaults to opaque black.
                    #[serde(default)]
                    color: Option<[u8; 4]>,
                    anchor: i64,
                }
                let r: Req = decode(payload)?;
                let text = build_text_props(r.content, r.style);
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
                    #[serde(flatten)]
                    style: StyleWire,
                    x: f64,
                    y: f64,
                    /// RGBA 0–255. Defaults to opaque black.
                    #[serde(default)]
                    color: Option<[u8; 4]>,
                }
                let r: Req = decode(payload)?;
                let text = build_text_props(r.content, r.style);
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
                    /// Axis values to **merge** (tag → value); the rest are kept.
                    #[serde(default)]
                    variations: Option<BTreeMap<String, f32>>,
                    #[serde(default)]
                    features: Option<BTreeMap<String, u32>>,
                    #[serde(default)]
                    letter_spacing: Option<f32>,
                    #[serde(default)]
                    word_spacing: Option<f32>,
                    #[serde(default)]
                    line_height: Option<f32>,
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
                    r.variations,
                    r.features,
                    r.letter_spacing,
                    r.word_spacing,
                    r.line_height,
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
                            "variations": o.variations,
                            "features": o.features,
                            "letter_spacing": o.letter_spacing,
                            "word_spacing": o.word_spacing,
                            "line_height": o.line_height,
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
        RequestRegistration {
            kind: "font_axes",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    family: String,
                }
                let r: Req = decode(payload)?;
                let caps = engine.font_axes(&r.family);
                let axes: Vec<_> = caps
                    .axes
                    .into_iter()
                    .map(|a| {
                        json!({
                            "tag": a.tag,
                            "min": a.min,
                            "default": a.default,
                            "max": a.max,
                        })
                    })
                    .collect();
                Ok(Response::json(
                    json!({ "italic": caps.italic, "axes": axes }),
                ))
            },
        },
        RequestRegistration {
            kind: "register_font",
            handle: |engine, _payload, bytes| {
                // Raw SFNT (`.ttf`/`.otf`) bytes arrive alongside the payload —
                // the same binary-in pattern as `brush_upload_image`. Returns
                // the family names the blob contributed so the frontend library
                // can index them.
                let families = engine.register_font(bytes.to_vec());
                Ok(Response::json(json!({ "families": families })))
            },
        },
    ]
}
