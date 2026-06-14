use crate::engine::protocol::{bad_payload, RequestRegistration, Response};

/// The full layer tree as a JSON array of `LayerInfo`. Query — no payload.
pub fn register() -> RequestRegistration {
    RequestRegistration {
        kind: "layer_tree",
        handle: |engine, _payload, _bytes| {
            let tree = engine.layer_tree();
            let value = serde_json::to_value(&tree).map_err(bad_payload)?;
            Ok(Response::json(value))
        },
    }
}
