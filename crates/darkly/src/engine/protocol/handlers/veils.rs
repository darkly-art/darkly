//! Veils (viewport post-process effects): list, mutate, and preview.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{decode, params_from_json, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "remove_veil",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    index: usize,
                }
                let r: Req = decode(payload)?;
                engine.remove_veil(r.index);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "clear_veils",
            handle: |engine, _payload, _b| {
                engine.clear_veils();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "set_veil_visible",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    index: usize,
                    visible: bool,
                }
                let r: Req = decode(payload)?;
                engine.set_veil_visible(r.index, r.visible);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "move_veil",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    from: usize,
                    to: usize,
                }
                let r: Req = decode(payload)?;
                engine.move_veil(r.from, r.to);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "add_veil",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    veil_type: String,
                    #[serde(default)]
                    params: serde_json::Value,
                }
                let r: Req = decode(payload)?;
                let pv = params_from_json(&r.params, engine.veil_param_defs(&r.veil_type));
                engine.add_veil(&r.veil_type, &pv);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "update_veil",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    index: usize,
                    #[serde(default)]
                    params: serde_json::Value,
                }
                let r: Req = decode(payload)?;
                // Resolve the veil's type id from its slot so we coerce params
                // against the right schema.
                let type_id = match engine.veil_list().iter().find(|v| v.index == r.index) {
                    Some(v) => v.type_id.clone(),
                    None => return Ok(Response::empty()),
                };
                let pv = params_from_json(&r.params, engine.veil_param_defs(&type_id));
                engine.update_veil(r.index, &pv);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "start_veil_preview",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    veil_type: String,
                }
                let r: Req = decode(payload)?;
                engine.start_veil_preview(&r.veil_type);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "poll_veil_preview",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    veil_type: String,
                }
                let r: Req = decode(payload)?;
                let Some((width, height, frames)) = engine.poll_veil_preview(&r.veil_type) else {
                    return Ok(Response::json(serde_json::Value::Null));
                };
                // Frames are concatenated into the single bytes side-channel;
                // the JS edge slices them back out using width*height*4 stride.
                let fps = crate::gpu::veil_preview::PREVIEW_FPS;
                let frame_count = frames.len();
                let mut bytes = Vec::new();
                for f in &frames {
                    bytes.extend_from_slice(f);
                }
                let value = json!({
                    "width": width,
                    "height": height,
                    "fps": fps,
                    "frameCount": frame_count,
                });
                Ok(Response::binary(value, bytes))
            },
        },
    ]
}
