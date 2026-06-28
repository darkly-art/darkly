//! Layer / group / node structural mutations — add, remove, move, duplicate,
//! group, merge, flatten, and void-layer param management.
//!
//! Every handler here is pure forwarding: decode a request whose fields name
//! the engine method's parameters one-for-one, call the method, wrap the
//! return. The decode does all the marshalling — `LayerId`, `MoveTarget`,
//! `OrthoXform`, and `Transform` are serde-native, and `Option<LayerId>`
//! carries the "no anchor" case as `null` — so there are no `*_from_ffi`
//! shims, sentinel ints, or hand-written `&str → variant` maps left. The one
//! thing decode can't do is pair a raw `params` object with the sibling field
//! that names its schema; that single seam is [`DarklyEngine::coerce_void_params`].

use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::protocol::{
    decode, params_from_json, ProtocolError, RawParams, RequestRegistration, Response,
};
use crate::gpu::ortho_transform::OrthoXform;
use crate::layer::LayerId;
use crate::transform::Transform;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "add_raster",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    #[serde(default)]
                    anchor: Option<LayerId>,
                }
                let r: Req = decode(payload)?;
                Ok(Response::json(
                    json!({ "id": engine.add_raster_layer(r.anchor) }),
                ))
            },
        },
        RequestRegistration {
            kind: "add_void",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    void_type: String,
                    #[serde(default)]
                    params: RawParams,
                    #[serde(default)]
                    anchor: Option<LayerId>,
                }
                let r: Req = decode(payload)?;
                let pv = engine.coerce_void_params(&r.void_type, &r.params.0);
                Ok(Response::json(
                    json!({ "id": engine.add_void_layer(&r.void_type, pv, r.anchor) }),
                ))
            },
        },
        RequestRegistration {
            kind: "add_filter_layer",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    pipeline: String,
                    #[serde(default)]
                    params: RawParams,
                    #[serde(default)]
                    anchor: Option<LayerId>,
                }
                let r: Req = decode(payload)?;
                // Filters carry no params today (invert is parameter-free), so
                // the schema is empty; `params_from_json` yields an empty vec.
                let pv = params_from_json(&r.params.0, &[]);
                Ok(Response::json(
                    json!({ "id": engine.add_filter_layer(&r.pipeline, pv, r.anchor) }),
                ))
            },
        },
        RequestRegistration {
            kind: "update_void_params",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: LayerId,
                    #[serde(default)]
                    params: RawParams,
                }
                let r: Req = decode(payload)?;
                let Some(type_id) = engine.void_layer_type(r.id) else {
                    return Ok(Response::empty());
                };
                let pv = engine.coerce_void_params(&type_id, &r.params.0);
                engine.update_void_params(r.id, pv);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "update_void_transform",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: LayerId,
                    transform: Transform,
                }
                let r: Req = decode(payload)?;
                engine.update_void_transform(r.id, r.transform);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "void_transform_info",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: LayerId,
                }
                let r: Req = decode(payload)?;
                let value = match engine.void_transform_info(r.id) {
                    Some((ox, oy, w, h, t)) => json!({
                        "ox": ox, "oy": oy, "w": w, "h": h,
                        "mode": t.mode_tag(), "matrix": t.to_affine(),
                    }),
                    None => Value::Null,
                };
                Ok(Response::json(value))
            },
        },
        RequestRegistration {
            kind: "layer_transform_capability",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: LayerId,
                }
                let r: Req = decode(payload)?;
                Ok(Response::json(
                    json!({ "value": engine.layer_transform_capability(r.id) }),
                ))
            },
        },
        RequestRegistration {
            kind: "remove_layer",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: LayerId,
                }
                let r: Req = decode(payload)?;
                engine.remove_layer(r.id).map_err(ProtocolError::engine)?;
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "duplicate_nodes",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    ids: Vec<LayerId>,
                }
                let r: Req = decode(payload)?;
                Ok(Response::json(
                    json!({ "ids": engine.duplicate_nodes(r.ids) }),
                ))
            },
        },
        RequestRegistration {
            kind: "merge_layers",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    ids: Vec<LayerId>,
                }
                let r: Req = decode(payload)?;
                let id = engine.merge_layers(r.ids).map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id })))
            },
        },
        RequestRegistration {
            kind: "duplicate_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    source_id: LayerId,
                }
                let r: Req = decode(payload)?;
                Ok(Response::json(
                    json!({ "id": engine.duplicate_node(r.source_id) }),
                ))
            },
        },
        RequestRegistration {
            kind: "flip_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: LayerId,
                    xform: OrthoXform,
                }
                let r: Req = decode(payload)?;
                Ok(Response::json(
                    json!({ "ok": engine.flip_node(r.node_id, r.xform) }),
                ))
            },
        },
        RequestRegistration {
            kind: "merge_down",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    source_id: LayerId,
                }
                let r: Req = decode(payload)?;
                let id = engine
                    .merge_down(r.source_id)
                    .map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id })))
            },
        },
        RequestRegistration {
            kind: "flatten_image",
            handle: |engine, _payload, _b| {
                let id = engine.flatten_image().map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id })))
            },
        },
        RequestRegistration {
            kind: "flatten_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: LayerId,
                }
                let r: Req = decode(payload)?;
                let id = engine
                    .flatten_node(r.node_id)
                    .map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id })))
            },
        },
        RequestRegistration {
            kind: "can_flatten_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: LayerId,
                }
                let r: Req = decode(payload)?;
                Ok(Response::json(
                    json!({ "value": engine.can_flatten_node(r.node_id) }),
                ))
            },
        },
        RequestRegistration {
            kind: "can_merge_down",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    source_id: LayerId,
                }
                let r: Req = decode(payload)?;
                Ok(Response::json(
                    json!({ "value": engine.can_merge_down(r.source_id) }),
                ))
            },
        },
        RequestRegistration {
            kind: "can_flatten",
            handle: |engine, _payload, _b| {
                Ok(Response::json(json!({ "value": engine.can_flatten() })))
            },
        },
    ]
}
