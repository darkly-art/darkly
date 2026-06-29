//! Tool-overlay primitive upload. The wire carries a camelCase `PrimIn`
//! that maps onto the `#[repr(C)]` GPU [`OverlayPrimitive`] (which is not
//! itself `Deserialize`), so this conversion can't be macro-derived and the
//! handler stays hand-written. The mask-texture and hit-test ops are
//! `#[handler]`-generated on `engine/filters/selection.rs`.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::gpu::overlay::OverlayPrimitive;

fn white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn one() -> f32 {
    1.0
}

/// JSON-side overlay primitive (camelCase to match the frontend). Maps onto the
/// `#[repr(C)]` GPU [`OverlayPrimitive`], which is not itself `Deserialize`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrimIn {
    kind: u32,
    #[serde(default)]
    flags: u32,
    p0: [f32; 2],
    p1: [f32; 2],
    #[serde(default = "white")]
    color: [f32; 4],
    #[serde(default = "one")]
    thickness: f32,
    #[serde(default)]
    dash_len: f32,
    #[serde(default)]
    dash_offset: f32,
    #[serde(default)]
    corner_radius: f32,
    #[serde(default)]
    mode_param: f32,
    #[serde(default)]
    rotation: f32,
}

impl From<PrimIn> for OverlayPrimitive {
    fn from(p: PrimIn) -> Self {
        OverlayPrimitive {
            color: p.color,
            p0: p.p0,
            p1: p.p1,
            thickness: p.thickness,
            dash_len: p.dash_len,
            dash_offset: p.dash_offset,
            corner_radius: p.corner_radius,
            kind: p.kind,
            flags: p.flags,
            mode_param: p.mode_param,
            rotation: p.rotation,
        }
    }
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![RequestRegistration {
        kind: "set_overlay",
        handle: |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                primitives: Vec<PrimIn>,
            }
            let r: Req = decode(payload)?;
            let prims = r
                .primitives
                .into_iter()
                .map(OverlayPrimitive::from)
                .collect();
            engine.set_overlay_primitives(prims);
            Ok(Response::empty())
        },
    }]
}
