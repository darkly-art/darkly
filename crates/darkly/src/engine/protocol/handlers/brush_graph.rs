//! Brush node-graph handlers the `#[handler]` macro can't derive. The
//! structural mutations (add/remove node, connect, expose, …), the type-list
//! queries, and `set_brush_blend_mode` are now generated on
//! `engine/brush_graph.rs`. What stays here:
//!
//! - **kind ≠ engine-method name** — the wire verb and the engine method differ
//!   (`brush_graph_compile` → `set_brush_graph`, `brush_graph_active` →
//!   `active_brush_graph`, …); the macro keys the kind off the method name.
//! - **value-shaping** — `{ yaml }` / `{ value }` envelopes, and the
//!   `null | { error }` compile/validate result (`returns = ok_error` would fit,
//!   but the kind/method mismatch keeps these hand-written anyway).
//! - **param marshalling** — `brush_graph_set_param` (kind/value → `ParamValue`)
//!   and `brush_graph_auto_layout` (`HashMap<u64,…>` ↔ `NodeId` keys).
//! - **`brush_upload_image`** — an always-`Err` stub with unused params.

use serde::Deserialize;
use serde_json::json;

use crate::engine::protocol::{
    bad_payload, decode, graph_result, ok_or_error, ProtocolError, RequestRegistration, Response,
};
use crate::gpu::params::ParamValue;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
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
    ]
}
