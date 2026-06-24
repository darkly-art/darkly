//! Registry / type-list queries — the introspection surface the frontend uses
//! to populate menus (tools, blend modes, veils, voids, modifiers, layer kinds)
//! and to render the layer tree. All read-only, all `serde::Serialize` lists.

use crate::engine::protocol::{bad_payload, RequestRegistration, Response};
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("layer_tree", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.layer_tree()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("veil_list", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.veil_list()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("veil_types", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.veil_types()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("void_types", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.void_types()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("adjustment_types", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.adjustment_types()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("tool_types", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.tool_types()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("blend_mode_types", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.blend_mode_types()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("modifier_types", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.modifier_types()).map_err(bad_payload)?,
            ))
        }),
        RequestRegistration::new::<Value, Value>("layer_kind_types", |engine, _payload, _b| {
            Ok(Response::json(
                serde_json::to_value(engine.layer_kind_types()).map_err(bad_payload)?,
            ))
        }),
    ]
}
