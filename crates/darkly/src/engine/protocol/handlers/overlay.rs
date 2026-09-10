//! Tool-overlay primitive upload. The wire carries a camelCase `PrimIn`
//! that maps onto the `#[repr(C)]` GPU [`OverlayPrimitive`] (which is not
//! itself `Deserialize`), so this conversion can't be macro-derived and the
//! handler stays hand-written. The mask-texture and hit-test ops are
//! `#[handler]`-generated on `engine/filters/selection.rs`.

use serde::Deserialize;

use crate::engine::protocol::{decode, RequestRegistration, Response};
use crate::engine::OverlayChannel;
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PrimIn {
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

/// `{ primitives }`: the full overlay primitive list to upload.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct SetOverlayReq {
    pub primitives: Vec<PrimIn>,
}

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new("set_overlay", |engine, payload, _b| {
            let r: SetOverlayReq = decode(payload)?;
            let prims = r
                .primitives
                .into_iter()
                .map(OverlayPrimitive::from)
                .collect();
            engine.set_overlay_primitives(prims);
            Ok(Response::empty())
        })
        .post()
        .req::<SetOverlayReq>(),
        // Clone-brush source marker / hint. Targets the `CloneSource`
        // channel, which persists across the dab preview's every-hover-move
        // replacement of the `Tool` channel.
        RequestRegistration::new("set_clone_overlay", |engine, payload, _b| {
            let r: SetOverlayReq = decode(payload)?;
            let prims = r
                .primitives
                .into_iter()
                .map(OverlayPrimitive::from)
                .collect();
            engine.set_channel_overlay(OverlayChannel::CloneSource, prims);
            Ok(Response::empty())
        })
        .post()
        .req::<SetOverlayReq>(),
        RequestRegistration::new("clear_clone_overlay", |engine, _payload, _b| {
            engine.clear_channel_overlay(OverlayChannel::CloneSource);
            Ok(Response::empty())
        })
        .post(),
    ]
}
