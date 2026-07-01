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

/// `{ json }` — a serialized node-graph (compile / validate).
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushGraphJsonReq {
    pub json: String,
}

/// `{ yaml }` — a YAML node-graph to import.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushGraphYamlReq {
    pub yaml: String,
}

/// `{ node_id, param_index, kind, value }` — set one node param. `value` is
/// interpreted per `kind` (`float`/`int`/`bool`/`string`/`curve`).
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushGraphSetParamReq {
    pub node_id: u64,
    pub param_index: usize,
    pub kind: String,
    #[cfg_attr(feature = "ts-export", ts(type = "JsonValue"))]
    pub value: serde_json::Value,
}

/// `{ sizes }` — measured node box sizes, keyed by node id, for auto-layout.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushGraphAutoLayoutReq {
    pub sizes: std::collections::HashMap<u64, [f32; 2]>,
}

/// `{ resource_name, width, height }` — an uploaded image resource; pixels ride
/// the binary side-channel.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct BrushUploadImageReq {
    pub resource_name: String,
    pub width: u32,
    pub height: u32,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("brush_graph_reset", |engine, _payload, _b| {
            engine.reset_brush_graph();
            Ok(Response::empty())
        })
        .post(),
        RequestRegistration::new("brush_graph_compile", |engine, payload, _b| {
            let r: BrushGraphJsonReq = decode(payload)?;
            Ok(ok_or_error(engine.set_brush_graph(&r.json)))
        })
        .send()
        .req::<BrushGraphJsonReq>()
        .resp_literal("null | { error: string }"),
        RequestRegistration::new("brush_graph_export_yaml", |engine, _payload, _b| {
            let yaml = engine.active_brush_graph_yaml().unwrap_or_default();
            Ok(Response::json(json!({ "yaml": yaml })))
        })
        .send()
        .resp_literal("{ yaml: string }"),
        RequestRegistration::new("brush_graph_import_yaml", |engine, payload, _b| {
            let r: BrushGraphYamlReq = decode(payload)?;
            Ok(ok_or_error(engine.set_brush_graph_yaml(&r.yaml)))
        })
        .send()
        .req::<BrushGraphYamlReq>()
        .resp_literal("null | { error: string }"),
        RequestRegistration::new("brush_graph_set_param", |engine, payload, _b| {
            let r: BrushGraphSetParamReq = decode(payload)?;
            let pv = match r.kind.as_str() {
                "float" => ParamValue::Float(r.value.as_f64().unwrap_or(0.0) as f32),
                "int" => ParamValue::Int(r.value.as_f64().unwrap_or(0.0) as i32),
                "bool" => ParamValue::Bool(r.value.as_bool().unwrap_or(false)),
                "string" => ParamValue::String(r.value.as_str().unwrap_or_default().to_string()),
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
        })
        .send()
        .req::<BrushGraphSetParamReq>()
        .resp_literal("{ graph: JsonValue } | { error: string }"),
        RequestRegistration::new("brush_graph_auto_layout", |engine, payload, _b| {
            let r: BrushGraphAutoLayoutReq = decode(payload)?;
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
        })
        .send()
        .req::<BrushGraphAutoLayoutReq>()
        .resp_literal("Record<string, [number, number]>"),
        RequestRegistration::new("brush_upload_image", |engine, payload, bytes| {
            let r: BrushUploadImageReq = decode(payload)?;
            match engine.brush_upload_image(&r.resource_name, r.width, r.height, bytes) {
                Ok(()) => Ok(Response::empty()),
                Err(e) => Err(ProtocolError::engine(e)),
            }
        })
        .send()
        .bytes_in()
        .req::<BrushUploadImageReq>(),
        RequestRegistration::new("brush_graph_default", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.default_brush_graph()).map_err(bad_payload)?,
            ))
        })
        .send()
        .resp_literal("JsonValue"),
        RequestRegistration::new("brush_graph_active", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.active_brush_graph()).map_err(bad_payload)?,
            ))
        })
        .send()
        .resp_literal("JsonValue"),
        RequestRegistration::new("brush_topology_version", |engine, _payload, _b| {
            Ok(Response::json(
                json!({ "value": engine.brush_topology_version() }),
            ))
        })
        .send()
        .resp_literal("{ value: number }"),
        RequestRegistration::new("brush_active_supports_erase", |engine, _payload, _b| {
            Ok(Response::json(
                json!({ "value": engine.active_brush_supports_erase() }),
            ))
        })
        .send()
        .resp_literal("{ value: boolean }"),
        RequestRegistration::new("brush_graph_validate", |engine, payload, _b| {
            let r: BrushGraphJsonReq = decode(payload)?;
            Ok(ok_or_error(engine.validate_brush_graph(&r.json)))
        })
        .send()
        .req::<BrushGraphJsonReq>()
        .resp_literal("null | { error: string }"),
    ]
}
