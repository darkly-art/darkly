//! Registry / type-list queries — the introspection surface the frontend uses
//! to populate menus (tools, blend modes, veils, voids, modifiers, layer kinds)
//! and to render the layer tree. All read-only, all `serde::Serialize` lists.

use crate::engine::protocol::{bad_payload, RequestRegistration, Response};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration {
            kind: "layer_tree",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.layer_tree()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "veil_list",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.veil_list()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "veil_types",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.veil_types()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "void_types",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.void_types()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "tool_types",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.tool_types()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "blend_mode_types",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.blend_mode_types()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "modifier_types",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.modifier_types()).map_err(bad_payload)?,
                ))
            },
        },
        RequestRegistration {
            kind: "layer_kind_types",
            handle: |engine, _payload, _b| {
                Ok(Response::json(
                    serde_json::to_value(engine.layer_kind_types()).map_err(bad_payload)?,
                ))
            },
        },
    ]
}
