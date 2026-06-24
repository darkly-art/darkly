//! Overlay primitives + the masked-stamp overlay texture.

use serde::Deserialize;
use serde_json::json;
use serde_json::Value;

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
    vec![
        RequestRegistration::new::<Value, Value>("set_overlay", |engine, payload, _b| {
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
        }),
        RequestRegistration::new::<Value, Value>("clear_overlay", |engine, _payload, _b| {
            engine.clear_overlay();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("set_overlay_mask", |engine, payload, bytes| {
            // RGBA8 mask uploaded via the binary side-channel; the red
            // channel is used as grayscale coverage.
            #[derive(Deserialize)]
            struct Req {
                width: u32,
                height: u32,
            }
            let r: Req = decode(payload)?;
            engine.set_overlay_mask(r.width, r.height, bytes);
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("clear_overlay_mask", |engine, _payload, _b| {
            engine.clear_overlay_mask();
            Ok(Response::empty())
        }),
        RequestRegistration::new::<Value, Value>("overlay_hit_test", |engine, payload, _b| {
            #[derive(Deserialize)]
            struct Req {
                screen_x: f32,
                screen_y: f32,
            }
            let r: Req = decode(payload)?;
            let v = engine
                .overlay_hit_test(r.screen_x, r.screen_y)
                .map(|i| i as i64)
                .unwrap_or(-1);
            Ok(Response::json(json!({ "value": v })))
        }),
    ]
}
