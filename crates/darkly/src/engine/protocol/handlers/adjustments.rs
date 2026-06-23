//! Destructive color-adjustment requests — apply a registered adjustment
//! (invert, …) to a node. The type list is exposed via `adjustment_types`
//! (see `registry_types.rs`); applying one is here.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![RequestRegistration {
        kind: "apply_adjustment",
        handle: |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                node_id: u64,
                adjustment_type: String,
            }
            let r: Req = decode(payload)?;
            // Deferred: spawns a task that warms the selection cache (if cold),
            // applies the adjustment, and resolves this request with `{ ok }`.
            engine.spawn_adjustment(LayerId::from_ffi(r.node_id), &r.adjustment_type);
            Ok(Response::deferred())
        },
    }]
}
