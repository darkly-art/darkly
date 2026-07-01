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

use serde::{Deserialize, Serialize};

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::layer::LayerId;

/// `{ id }` selecting the void layer to query.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct VoidTransformInfoReq {
    pub id: LayerId,
}

/// Flat transform info for a void layer — `mode`/`matrix` derived from the
/// engine's `Transform`.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct VoidTransformInfoResp {
    pub ox: f32,
    pub oy: f32,
    pub w: f32,
    pub h: f32,
    pub mode: u32,
    pub matrix: [f32; 6],
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("void_transform_info", |engine, payload, _b| {
            let r: VoidTransformInfoReq = decode(payload)?;
            let value = match engine.void_transform_info(r.id) {
                Some((ox, oy, w, h, t)) => serde_json::to_value(VoidTransformInfoResp {
                    ox,
                    oy,
                    w,
                    h,
                    mode: t.mode_tag(),
                    matrix: t.to_affine(),
                })
                .map_err(crate::engine::protocol::bad_payload)?,
                None => serde_json::Value::Null,
            };
            Ok(Response::json(value))
        })
        .send()
        .req::<VoidTransformInfoReq>()
        .resp::<Option<VoidTransformInfoResp>>(),
    ]
}
