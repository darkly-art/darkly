use serde_json::json;

use crate::engine::protocol::{bad_payload, RequestRegistration, Response};
use crate::layer::LayerId;

/// Add a raster layer above `anchor` (negative = no anchor / top of root).
/// Mirrors the `anchor_id >= 0` "negative = none" FFI convention.
pub fn register() -> RequestRegistration {
    RequestRegistration {
        kind: "add_raster",
        handle: |engine, payload, _bytes| {
            #[derive(serde::Deserialize)]
            struct Req {
                anchor: i64,
            }
            let req: Req = serde_json::from_value(payload).map_err(bad_payload)?;
            let anchor = (req.anchor >= 0).then(|| LayerId::from_ffi(req.anchor as u64));
            let id = engine.add_raster_layer(anchor);
            Ok(Response::json(json!({ "id": id.to_ffi() })))
        },
    }
}
