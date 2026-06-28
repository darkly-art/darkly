//! Layer / group / node structural mutations — add, remove, move, duplicate,
//! group, merge, flatten, and void-layer param management.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{
    decode, layer_id, params_from_json, ProtocolError, RequestRegistration, Response,
};
use crate::gpu::params::ParamDef;
use crate::layer::LayerId;

/// Build a [`crate::document::MoveTarget`] from the wire `{ target_type, target_id }`.
fn move_target(t: &str, id: LayerId) -> Result<crate::document::MoveTarget, ProtocolError> {
    use crate::document::MoveTarget::*;
    Ok(match t {
        "before" => Before(id),
        "after" => After(id),
        "into_top" => IntoGroupTop(id),
        "into_bottom" => IntoGroupBottom(id),
        _ => return Err(ProtocolError::engine("unknown move target")),
    })
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "add_raster",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    anchor: i64,
                }
                let r: Req = decode(payload)?;
                let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
                Ok(Response::json(
                    json!({ "id": engine.add_raster_layer(anchor).to_ffi() }),
                ))
            },
        },
        RequestRegistration {
            kind: "add_group",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    anchor: i64,
                }
                let r: Req = decode(payload)?;
                let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
                Ok(Response::json(
                    json!({ "id": engine.add_group(anchor).to_ffi() }),
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
                    params: serde_json::Value,
                    anchor: i64,
                }
                let r: Req = decode(payload)?;
                let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
                let defs: &'static [ParamDef] = engine.void_param_defs(&r.void_type);
                let pv = params_from_json(&r.params, defs);
                let value = match engine.add_void_layer(&r.void_type, pv, anchor) {
                    Some(id) => json!({ "id": id.to_ffi() }),
                    None => json!({ "id": -1 }),
                };
                Ok(Response::json(value))
            },
        },
        RequestRegistration {
            kind: "add_filter_layer",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    pipeline: String,
                    #[serde(default)]
                    params: serde_json::Value,
                    anchor: i64,
                }
                let r: Req = decode(payload)?;
                let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
                // Filters carry no params today (invert is parameter-free), so
                // the schema is empty; `params_from_json` yields an empty vec.
                let pv = params_from_json(&r.params, &[]);
                let value = match engine.add_filter_layer(&r.pipeline, pv, anchor) {
                    Some(id) => json!({ "id": id.to_ffi() }),
                    None => json!({ "id": -1 }),
                };
                Ok(Response::json(value))
            },
        },
        RequestRegistration {
            kind: "update_void_params",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    params: serde_json::Value,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                let type_id = match engine.void_layer_type(id) {
                    Some(t) => t,
                    None => return Ok(Response::empty()),
                };
                let defs = engine.void_param_defs(&type_id);
                let pv = params_from_json(&r.params, defs);
                engine.update_void_params(id, pv);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "update_void_transform",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    #[serde(default)]
                    mode_tag: u32,
                    payload: Vec<f32>,
                }
                let r: Req = decode(payload)?;
                let id = LayerId::from_ffi(r.id);
                // Shared decoder with the floating path. Voids only ever
                // receive tag 0 (affine) today; perspective for voids is a
                // documented follow-up. Unknown tags / short payloads no-op.
                if let Some(t) =
                    crate::transform::Transform::from_tag_payload(r.mode_tag, &r.payload)
                {
                    engine.update_void_transform(id, t);
                }
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "void_transform_info",
            handle: |engine, payload, _b| {
                let id = layer_id(payload)?;
                let value = match engine.void_transform_info(id) {
                    Some((ox, oy, w, h, t)) => json!({
                        "ox": ox, "oy": oy, "w": w, "h": h,
                        "mode": t.mode_tag(), "matrix": t.to_affine(),
                    }),
                    None => serde_json::Value::Null,
                };
                Ok(Response::json(value))
            },
        },
        RequestRegistration {
            kind: "layer_transform_capability",
            handle: |engine, payload, _b| {
                let id = layer_id(payload)?;
                Ok(Response::json(
                    json!({ "value": engine.layer_transform_capability(id) }),
                ))
            },
        },
        RequestRegistration {
            kind: "remove_layer",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                }
                let r: Req = decode(payload)?;
                engine
                    .remove_layer(LayerId::from_ffi(r.id))
                    .map_err(ProtocolError::engine)?;
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "move_layer",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    id: u64,
                    target_type: String,
                    target_id: u64,
                }
                let r: Req = decode(payload)?;
                let target = move_target(&r.target_type, LayerId::from_ffi(r.target_id))?;
                engine.move_layer(LayerId::from_ffi(r.id), target);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "remove_layers",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    ids: Vec<u64>,
                }
                let r: Req = decode(payload)?;
                let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
                let n = engine.remove_layers(ids).map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "skipped": n })))
            },
        },
        RequestRegistration {
            kind: "move_layers",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    ids: Vec<u64>,
                    target_type: String,
                    target_id: u64,
                }
                let r: Req = decode(payload)?;
                let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
                let target = move_target(&r.target_type, LayerId::from_ffi(r.target_id))?;
                let n = engine
                    .move_layers(ids, target)
                    .map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "skipped": n })))
            },
        },
        RequestRegistration {
            kind: "duplicate_nodes",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    ids: Vec<u64>,
                }
                let r: Req = decode(payload)?;
                let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
                let out: Vec<u64> = engine
                    .duplicate_nodes(ids)
                    .into_iter()
                    .map(|id| id.to_ffi())
                    .collect();
                Ok(Response::json(json!({ "ids": out })))
            },
        },
        RequestRegistration {
            kind: "group_layers",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    ids: Vec<u64>,
                }
                let r: Req = decode(payload)?;
                let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
                let id = engine.group_layers(ids).map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id.to_ffi() })))
            },
        },
        RequestRegistration {
            kind: "merge_layers",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    ids: Vec<u64>,
                }
                let r: Req = decode(payload)?;
                let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
                let id = engine.merge_layers(ids).map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id.to_ffi() })))
            },
        },
        RequestRegistration {
            kind: "duplicate_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    source_id: u64,
                }
                let r: Req = decode(payload)?;
                let out = engine
                    .duplicate_node(LayerId::from_ffi(r.source_id))
                    .map(|n| n.to_ffi())
                    .unwrap_or(0);
                Ok(Response::json(json!({ "id": out })))
            },
        },
        RequestRegistration {
            kind: "flip_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                #[serde(rename_all = "lowercase")]
                enum Axis {
                    H,
                    V,
                }
                #[derive(Deserialize)]
                struct Req {
                    node_id: u64,
                    axis: Axis,
                }
                let r: Req = decode(payload)?;
                let ok = engine.flip_node(
                    LayerId::from_ffi(r.node_id),
                    match r.axis {
                        Axis::H => crate::gpu::ortho_transform::OrthoXform::FlipH,
                        Axis::V => crate::gpu::ortho_transform::OrthoXform::FlipV,
                    },
                );
                Ok(Response::json(json!({ "ok": ok })))
            },
        },
        RequestRegistration {
            kind: "merge_down",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    source_id: u64,
                }
                let r: Req = decode(payload)?;
                let id = engine
                    .merge_down(LayerId::from_ffi(r.source_id))
                    .map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id.to_ffi() })))
            },
        },
        RequestRegistration {
            kind: "flatten_image",
            handle: |engine, _payload, _b| {
                let id = engine.flatten_image().map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id.to_ffi() })))
            },
        },
        RequestRegistration {
            kind: "flatten_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: u64,
                }
                let r: Req = decode(payload)?;
                let id = engine
                    .flatten_node(LayerId::from_ffi(r.node_id))
                    .map_err(ProtocolError::engine)?;
                Ok(Response::json(json!({ "id": id.to_ffi() })))
            },
        },
        RequestRegistration {
            kind: "can_flatten_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: u64,
                }
                let r: Req = decode(payload)?;
                let v = engine.can_flatten_node(LayerId::from_ffi(r.node_id));
                Ok(Response::json(json!({ "value": v })))
            },
        },
        RequestRegistration {
            kind: "can_merge_down",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    source_id: u64,
                }
                let r: Req = decode(payload)?;
                let v = engine.can_merge_down(LayerId::from_ffi(r.source_id));
                Ok(Response::json(json!({ "value": v })))
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
