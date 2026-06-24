//! Veils (viewport post-process effects): list, mutate, and preview.

use serde::Deserialize;

use crate::engine::protocol::{decode, params_from_json, RequestRegistration, Response};
use serde_json::Value;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new::<Value, Value>("remove_veil", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                index: usize,
            }
            let r: Req = decode(payload)?;
            engine.remove_veil(r.index);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("clear_veils", |engine, _payload, _b| {
            engine.clear_veils();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_veil_visible", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                index: usize,
                visible: bool,
            }
            let r: Req = decode(payload)?;
            engine.set_veil_visible(r.index, r.visible);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("move_veil", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                from: usize,
                to: usize,
            }
            let r: Req = decode(payload)?;
            engine.move_veil(r.from, r.to);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("add_veil", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("update_veil", |engine, payload, _b| {
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
        }),
    ]
}
