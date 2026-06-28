//! Layer / group / node structural requests that can't be `#[handler]`-derived.
//!
//! The rest of this domain (add / remove / move / duplicate / group / merge /
//! flatten / void params) is generated from the engine methods themselves —
//! tag a method `#[handler]` (see `crate::engine::layers` and friends), no entry
//! here. What remains is the one query whose engine return can't serialize
//! straight to the wire:
//!
//! - **`void_transform_info`** returns a composite `(ox, oy, w, h, Transform)`
//!   tuple that four engine tests destructure as-is. The wire wants a flat
//!   `{ ox, oy, w, h, mode, matrix }` (with `mode`/`matrix` *derived* from the
//!   `Transform`), so the shaping lives here rather than polluting the engine
//!   method's natural return type. The macro's "the signature is the wire" rule
//!   genuinely doesn't fit, which is exactly when a hand-written handler earns
//!   its keep.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;

pub fn registrations() -> Vec<RequestRegistration> {
    vec![RequestRegistration {
        kind: "void_transform_info",
        handle: |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                id: LayerId,
            }
            let r: Req = decode(payload)?;
            let value = match engine.void_transform_info(r.id) {
                Some((ox, oy, w, h, t)) => json!({
                    "ox": ox, "oy": oy, "w": w, "h": h,
                    "mode": t.mode_tag(), "matrix": t.to_affine(),
                }),
                None => Value::Null,
            };
            Ok(Response::json(value))
        },
    }]
}
