//! Layer / group / node property mutations (all fire-and-forget).

use serde::Deserialize;

use crate::engine::protocol::{bad_payload, RequestRegistration, Response};
use crate::layer::LayerId;
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("set_opacity", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                opacity: f32,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            engine.set_opacity(LayerId::from_ffi(r.id), r.opacity);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_blend_mode", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                type_id: String,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            engine.set_blend_mode(LayerId::from_ffi(r.id), &r.type_id);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_layer_visible", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                visible: bool,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            engine.set_layer_visible(LayerId::from_ffi(r.id), r.visible);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_layer_name", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                name: String,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            engine.set_layer_name(LayerId::from_ffi(r.id), &r.name);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_group_collapsed", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                collapsed: bool,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            engine.set_group_collapsed(LayerId::from_ffi(r.id), r.collapsed);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_group_passthrough", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                passthrough: bool,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            engine.set_group_passthrough(LayerId::from_ffi(r.id), r.passthrough);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_node_locked", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: u64,
                locked: bool,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            engine.set_node_locked(LayerId::from_ffi(r.id), r.locked);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_isolated_node", |engine, payload, _b| {
            // `id == 0` is the JS-side sentinel for "clear isolation".
            #[derive(Deserialize)]
            struct Req {
                id: u64,
            }
            let r: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            let target = (r.id != 0).then(|| LayerId::from_ffi(r.id));
            engine.set_isolated_node(target);
            Ok(Response::empty())
        }),
    ]
}
