//! Screenshare void: a live `getDisplayMedia()` capture as a layer.
//!
//! Sibling of the camera void: a thin config over the shared
//! [`crate::gpu::textured_void`] machinery. The only differences from the
//! camera are metadata (name, icon), the capture API ([`CaptureKind::Display`]
//! → `getDisplayMedia`), and the seeded transform (identity: a shared screen
//! has no natural "selfie" orientation to flip).

use crate::gpu::textured_void::ContentFit;
use crate::gpu::textured_void::{self, TexturedVoidConfig};
use crate::gpu::void::{CaptureKind, ParamDef, VoidRegistration, VoidSource};

pub const TYPE_ID: &str = "screenshare";

// Identical schema to the camera: the shared machinery looks these up by name,
// so the two stay decoupled even though they happen to match today.
const PARAMS: &[ParamDef] = &[
    // Freeze on the last received frame; suppresses uploads (GPU holds the
    // last frame) while keeping the share open: stopping a getDisplayMedia
    // track would end the share permanently, so freeze must not close it.
    ParamDef::boolean("freeze", false)
        .with_label("Freeze")
        .with_description("Holds the last captured frame instead of following the live feed."),
    // rAF frames to skip between capture → GPU uploads (see camera void).
    ParamDef::int("frame_divisor", 1, 60, 4)
        .with_label("Frame Skip")
        .with_description("Capture one frame in this many, to lighten the load."),
];

static CONFIG: TexturedVoidConfig = TexturedVoidConfig {
    type_id: TYPE_ID,
    display_name: "Screen Share",
    description: "Live frames from a shared screen, window or browser tab.",
    icon: "tabler:screen-share",
    params: PARAMS,
    source: VoidSource::Capture {
        capture: CaptureKind::Display,
    },
    fit: ContentFit::Cover,
    default_transform: |_, _| crate::transform::Transform::identity(),
};

pub fn register() -> VoidRegistration {
    textured_void::registration(&CONFIG, |params, shared| {
        textured_void::build_void(&CONFIG, params, shared)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::params::ParamValue;

    #[test]
    fn config_matches_type_id() {
        assert_eq!(CONFIG.type_id, TYPE_ID);
        let reg = register();
        assert_eq!(reg.type_id, "screenshare");
        assert_eq!(reg.display_name, "Screen Share");
        assert_eq!(reg.source.capture_kind(), Some(CaptureKind::Display));
    }

    #[test]
    fn seeds_identity_transform() {
        let t = (CONFIG.default_transform)(200, 100);
        assert_eq!(t, crate::transform::Transform::identity());
    }

    #[test]
    fn default_params_round_trip() {
        // freeze off, divisor 4: the schema's declared defaults, which is what
        // the picker sends and what `param_values` must echo back.
        let reg = register();
        let defaults: Vec<ParamValue> = reg.params.iter().map(|d| d.default_value()).collect();
        assert_eq!(defaults.len(), 2);
        assert_eq!(defaults[0], ParamValue::Bool(false));
        assert_eq!(defaults[1], ParamValue::Int(4));
    }
}
