//! Brush node-graph: config, structural mutations, params, exposed ports, and
//! graph/topology queries. Structural mutations return the recompiled graph via
//! [`graph_result`] (`{ graph } | { error }`); compile/validate/import use
//! [`ok_or_error`] (`null | { error }`).

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{
    bad_payload, decode, graph_result, ok_or_error, ProtocolError, RequestRegistration, Response,
};
use crate::gpu::params::ParamValue;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "set_brush_blend_mode",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    mode: u32,
                }
                let r: Req = decode(payload)?;
                engine.set_brush_blend_mode(r.mode);
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "brush_graph_reset",
            handle: |engine, _payload, _b| {
                engine.reset_brush_graph();
                Ok(Response::empty())
            },
        },
        RequestRegistration {
            kind: "brush_graph_compile",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    json: String,
                }
                let r: Req = decode(payload)?;
                Ok(ok_or_error(engine.set_brush_graph(&r.json)))
            },
        },
        RequestRegistration {
            kind: "brush_graph_export_yaml",
            handle: |engine, _payload, _b| {
                let yaml = engine.active_brush_graph_yaml().unwrap_or_default();
                Ok(Response::json(json!({ "yaml": yaml })))
            },
        },
        RequestRegistration {
            kind: "brush_graph_import_yaml",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    yaml: String,
                }
                let r: Req = decode(payload)?;
                Ok(ok_or_error(engine.set_brush_graph_yaml(&r.yaml)))
            },
        },
        RequestRegistration {
            kind: "brush_graph_add_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    type_id: String,
                }
                let r: Req = decode(payload)?;
                graph_result(engine.brush_graph_add_node(&r.type_id))
            },
        },
        RequestRegistration {
            kind: "brush_graph_remove_node",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: u64,
                }
                let r: Req = decode(payload)?;
                graph_result(engine.brush_graph_remove_node(r.node_id))
            },
        },
        RequestRegistration {
            kind: "brush_graph_connect",
            handle: |engine, payload, _b| {
                let r: Conn = decode(payload)?;
                graph_result(engine.brush_graph_connect(
                    r.from_node,
                    &r.from_port,
                    r.to_node,
                    &r.to_port,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_graph_disconnect",
            handle: |engine, payload, _b| {
                let r: Conn = decode(payload)?;
                graph_result(engine.brush_graph_disconnect(
                    r.from_node,
                    &r.from_port,
                    r.to_node,
                    &r.to_port,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_graph_set_param",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: u64,
                    param_index: usize,
                    kind: String,
                    value: serde_json::Value,
                }
                let r: Req = decode(payload)?;
                let pv = match r.kind.as_str() {
                    "float" => ParamValue::Float(r.value.as_f64().unwrap_or(0.0) as f32),
                    "int" => ParamValue::Int(r.value.as_f64().unwrap_or(0.0) as i32),
                    "bool" => ParamValue::Bool(r.value.as_bool().unwrap_or(false)),
                    "string" => {
                        ParamValue::String(r.value.as_str().unwrap_or_default().to_string())
                    }
                    "curve" => {
                        // Accept a real JSON array of [x, y] pairs (protocol
                        // native) or a JSON-encoded string (legacy shape).
                        let points = serde_json::from_value::<Vec<[f32; 2]>>(r.value.clone())
                            .ok()
                            .or_else(|| r.value.as_str().and_then(|s| serde_json::from_str(s).ok()))
                            .unwrap_or_else(|| vec![[0.0, 0.0], [1.0, 1.0]]);
                        ParamValue::Curve(points)
                    }
                    other => {
                        return graph_result(Err(format!("unknown param kind: {other}")));
                    }
                };
                graph_result(engine.brush_graph_set_param(r.node_id, r.param_index, pv))
            },
        },
        RequestRegistration {
            kind: "brush_graph_set_port_default",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: u64,
                    port_name: String,
                    value: f32,
                }
                let r: Req = decode(payload)?;
                graph_result(engine.brush_graph_set_port_default(r.node_id, &r.port_name, r.value))
            },
        },
        RequestRegistration {
            kind: "brush_graph_auto_layout",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    sizes: std::collections::HashMap<u64, [f32; 2]>,
                }
                let r: Req = decode(payload)?;
                let sizes = r
                    .sizes
                    .into_iter()
                    .map(|(id, wh)| (crate::nodegraph::NodeId(id), wh))
                    .collect();
                let layout = engine.brush_graph_auto_layout(&sizes);
                let out: std::collections::HashMap<String, [f32; 2]> = layout
                    .into_iter()
                    .map(|(id, pos)| (id.0.to_string(), pos))
                    .collect();
                Ok(Response::json(
                    serde_json::to_value(out).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_set_exposed_port",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    node_id: u64,
                    port_name: String,
                    display_value: f32,
                }
                let r: Req = decode(payload)?;
                graph_result(engine.brush_set_exposed_port(
                    r.node_id,
                    &r.port_name,
                    r.display_value,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_graph_expose_port",
            handle: |engine, payload, _b| {
                let r: PortRef = decode(payload)?;
                graph_result(engine.brush_graph_expose_port(r.node_id, &r.port_name))
            },
        },
        RequestRegistration {
            kind: "brush_graph_unexpose_port",
            handle: |engine, payload, _b| {
                let r: PortRef = decode(payload)?;
                graph_result(engine.brush_graph_unexpose_port(r.node_id, &r.port_name))
            },
        },
        RequestRegistration {
            kind: "brush_graph_set_exposed_port_meta",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    key: String,
                    label: String,
                    description: String,
                    icon: String,
                }
                let r: Req = decode(payload)?;
                graph_result(engine.brush_graph_set_exposed_port_meta(
                    &r.key,
                    r.label,
                    r.description,
                    r.icon,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_graph_reorder_exposed_port",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    key: String,
                    new_index: u32,
                }
                let r: Req = decode(payload)?;
                graph_result(engine.brush_graph_reorder_exposed_port(&r.key, r.new_index))
            },
        },
        RequestRegistration {
            kind: "brush_upload_image",
            handle: |engine, payload, bytes| {
                #[derive(Deserialize)]
                struct Req {
                    resource_name: String,
                    width: u32,
                    height: u32,
                }
                let r: Req = decode(payload)?;
                match engine.brush_upload_image(&r.resource_name, r.width, r.height, bytes) {
                    Ok(()) => Ok(Response::empty()),
                    Err(e) => Err(ProtocolError::engine(e)),
                }
            },
        },
        // --- queries ---
        RequestRegistration {
            kind: "brush_node_types",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.brush_node_types()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_graph_default",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.default_brush_graph()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_graph_active",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.active_brush_graph()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "brush_topology_version",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    json!({ "value": engine.brush_topology_version() }),
                ))
            },
        },
        RequestRegistration {
            kind: "brush_active_supports_erase",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    json!({ "value": engine.active_brush_supports_erase() }),
                ))
            },
        },
        RequestRegistration {
            kind: "brush_graph_validate",
            handle: |engine, payload, _b| {
                #[derive(Deserialize)]
                struct Req {
                    json: String,
                }
                let r: Req = decode(payload)?;
                Ok(ok_or_error(engine.validate_brush_graph(&r.json)))
            },
        },
        RequestRegistration {
            kind: "brush_exposed_ports",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.brush_exposed_ports()).map_err(bad_payload)?,
                ))
            },
        },
    ]
}

#[derive(Deserialize)]
struct Conn {
    from_node: u64,
    from_port: String,
    to_node: u64,
    to_port: String,
}

#[derive(Deserialize)]
struct PortRef {
    node_id: u64,
    port_name: String,
}
