//! Layer / group / node structural mutations — add, remove, move, duplicate,
//! group, merge, flatten, and void-layer param management.

use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

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
        RequestRegistration::new::<Value, Value>("add_raster", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                anchor: i64,
            }
            let r: Req = decode(payload)?;
            let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
            Ok(Response::json(
                json!({ "id": engine.add_raster_layer(anchor).to_ffi() }),
            ))
        }),
        RequestRegistration::new::<Value, Value>("add_group", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                anchor: i64,
            }
            let r: Req = decode(payload)?;
            let anchor = (r.anchor >= 0).then(|| LayerId::from_ffi(r.anchor as u64));
            Ok(Response::json(
                json!({ "id": engine.add_group(anchor).to_ffi() }),
            ))
        }),
        RequestRegistration::new::<Value, Value>("add_void", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("update_void_params", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("update_void_transform", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                #[serde(default)]
                mode_tag: u32,
                payload: Vec<f32>,
            }
            let r: Req = decode(payload)?;
            let id = LayerId::from_ffi(r.id);
            // Basic mode (tag 0) carries the 6 affine components. Unknown
            // modes / short payloads are ignored (no-op).
            if r.mode_tag == 0 && r.payload.len() >= 6 {
                let t = crate::transform::Transform::from_affine([
                    r.payload[0],
                    r.payload[1],
                    r.payload[2],
                    r.payload[3],
                    r.payload[4],
                    r.payload[5],
                ]);
                engine.update_void_transform(id, t);
            }
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("void_transform_info", |engine, payload, _b| {
            let id = layer_id(payload)?;
            let value = match engine.void_transform_info(id) {
                Some((ox, oy, w, h, t)) => json!({
                    "ox": ox, "oy": oy, "w": w, "h": h,
                    "mode": t.mode_tag(), "matrix": t.to_affine(),
                }),
                None => serde_json::Value::Null,
            };
            Ok(Response::json(value))
        }),
        RequestRegistration::new::<Value, Value>(
            "layer_transform_capability",
            |engine, payload, _b| {
                let id = layer_id(payload)?;
                Ok(Response::json(
                    json!({ "value": engine.layer_transform_capability(id) }),
                ))
            },
        ),
        RequestRegistration::new::<Value, Value>("remove_layer", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
            }
            let r: Req = decode(payload)?;
            engine
                .remove_layer(LayerId::from_ffi(r.id))
                .map_err(ProtocolError::engine)?;
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("move_layer", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("remove_layers", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                ids: Vec<u64>,
            }
            let r: Req = decode(payload)?;
            let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
            let n = engine.remove_layers(ids).map_err(ProtocolError::engine)?;
            Ok(Response::json(json!({ "skipped": n })))
        }),
        RequestRegistration::new::<Value, Value>("move_layers", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("duplicate_nodes", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("group_layers", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                ids: Vec<u64>,
            }
            let r: Req = decode(payload)?;
            let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
            let id = engine.group_layers(ids).map_err(ProtocolError::engine)?;
            Ok(Response::json(json!({ "id": id.to_ffi() })))
        }),
        RequestRegistration::new::<Value, Value>("merge_layers", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                ids: Vec<u64>,
            }
            let r: Req = decode(payload)?;
            let ids: Vec<LayerId> = r.ids.into_iter().map(LayerId::from_ffi).collect();
            let id = engine.merge_layers(ids).map_err(ProtocolError::engine)?;
            Ok(Response::json(json!({ "id": id.to_ffi() })))
        }),
        RequestRegistration::new::<Value, Value>("duplicate_node", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("flip_node", |engine, payload, _b| {
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
            // Deferred: spawns a task that warms the selection cache (if
            // cold), flips, and resolves this request with `{ ok }`.
            engine.spawn_flip(
                LayerId::from_ffi(r.node_id),
                match r.axis {
                    Axis::H => crate::gpu::ortho_transform::OrthoXform::FlipH,
                    Axis::V => crate::gpu::ortho_transform::OrthoXform::FlipV,
                },
            );
            Ok(Response::deferred())
        }),
        RequestRegistration::new::<Value, Value>("merge_down", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                source_id: u64,
            }
            let r: Req = decode(payload)?;
            let id = engine
                .merge_down(LayerId::from_ffi(r.source_id))
                .map_err(ProtocolError::engine)?;
            Ok(Response::json(json!({ "id": id.to_ffi() })))
        }),
        RequestRegistration::new::<Value, Value>("flatten_image", |engine, _payload, _b| {
            let id = engine.flatten_image().map_err(ProtocolError::engine)?;
            Ok(Response::json(json!({ "id": id.to_ffi() })))
        }),
        RequestRegistration::new::<Value, Value>("flatten_node", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                node_id: u64,
            }
            let r: Req = decode(payload)?;
            let id = engine
                .flatten_node(LayerId::from_ffi(r.node_id))
                .map_err(ProtocolError::engine)?;
            Ok(Response::json(json!({ "id": id.to_ffi() })))
        }),
        RequestRegistration::new::<Value, Value>("can_flatten_node", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                node_id: u64,
            }
            let r: Req = decode(payload)?;
            let v = engine.can_flatten_node(LayerId::from_ffi(r.node_id));
            Ok(Response::json(json!({ "value": v })))
        }),
        RequestRegistration::new::<Value, Value>("can_merge_down", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                source_id: u64,
            }
            let r: Req = decode(payload)?;
            let v = engine.can_merge_down(LayerId::from_ffi(r.source_id));
            Ok(Response::json(json!({ "value": v })))
        }),
        RequestRegistration::new::<Value, Value>("can_flatten", |engine, _payload, _b| {
            Ok(Response::json(json!({ "value": engine.can_flatten() })))
        }),
    ]
}
