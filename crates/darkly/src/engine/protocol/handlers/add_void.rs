use serde_json::{json, Value};

use crate::engine::protocol::{bad_payload, params_from_json, RequestRegistration, Response};
use crate::layer::LayerId;

/// Add a void layer of `void_type` seeded from `params` (a ParamDef-driven
/// object). Returns `{ id }`, or `{ id: -1 }` if `void_type` is unknown.
pub fn register() -> RequestRegistration {
    RequestRegistration {
        kind: "add_void",
        handle: |engine, payload, _bytes| {
            #[derive(serde::Deserialize)]
            struct Req {
                void_type: String,
                #[serde(default)]
                params: Value,
                anchor: i64,
            }
            let req: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            let anchor = (req.anchor >= 0).then(|| LayerId::from_ffi(req.anchor as u64));
            let defs = engine.void_param_defs(&req.void_type);
            let pv = params_from_json(&req.params, defs);
            match engine.add_void_layer(&req.void_type, pv, anchor) {
                Some(id) => Ok(Response::json(json!({ "id": id.to_ffi() }))),
                None => Ok(Response::json(json!({ "id": -1 }))),
            }
        },
    }
}
