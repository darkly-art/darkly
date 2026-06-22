//! Camera void — live webcam as a layer.
//!
//! A thin config over the shared [`crate::gpu::video_stream_void`] machinery.
//! Everything about the "live video stream → texture" pipeline lives there;
//! this file only declares what makes the camera *the camera*: its id, display
//! name, icon, capture kind ([`CaptureKind::Camera`] → `getUserMedia`), and a
//! seeded horizontal-flip transform so it opens in selfie view.

use crate::gpu::video_stream_void::{self, VideoStreamConfig};
use crate::gpu::void::{CaptureKind, ParamDef, VoidRegistration};

pub const TYPE_ID: &str = "camera";

// Scale / rotation / pan / mirror are NOT params — they're the generic
// [`crate::transform::Transform`] on the void layer, edited by the on-canvas
// gizmo (the Transform tool). Mirroring is a negative scale in that affine, and
// the camera seeds a horizontal flip at creation (see CONFIG.default_transform).
// Only the discrete freeze toggle and the upload throttle remain as params.
const PARAMS: &[ParamDef] = &[
    // Freeze the layer on its last received frame. When on, the void stops
    // accepting external image uploads (`wants_external_input` returns false)
    // so the GPU holds the last frame, and the JS-side `MediaStreamSource`
    // suppresses uploads — but keeps the stream open so toggling back off
    // resumes the live feed instantly (no re-prompt).
    ParamDef::Bool {
        name: "freeze",
        default: false,
    },
    // How many rAF frames to skip between webcam → GPU uploads. 1 = upload
    // every frame (live 60fps), 4 = upload every 4th frame (~15fps at 60Hz
    // rAF, the default). Higher values trade smoothness for GPU/CPU savings —
    // each skipped tick avoids a JS `drawImage` blit, a
    // `copy_external_image_to_texture`, the void's canvas-resolution fragment
    // shader, and the full compositor re-encode that an upload would trigger.
    // The JS-side `MediaStreamSource.tick()` reads this value from the layer
    // params and gates its own upload accordingly; nothing here reads the field
    // at render time.
    ParamDef::Int {
        name: "frame_divisor",
        min: 1,
        max: 60,
        default: 4,
    },
];

/// Horizontal flip about the canvas center — the selfie view every video-call
/// app shows, because the webcam points at the user and they expect their
/// reflection, not a backwards view. It's just the affine the shader already
/// samples through (no extra shader/uniform work), it round-trips through
/// save/load, and the user can drag a scale handle past the anchor to un-flip.
fn selfie_flip(canvas_w: u32, _canvas_h: u32) -> crate::transform::Transform {
    // x' = canvas_w - x, y' = y.
    crate::transform::Transform::from_affine([-1.0, 0.0, canvas_w as f32, 0.0, 1.0, 0.0])
}

static CONFIG: VideoStreamConfig = VideoStreamConfig {
    type_id: TYPE_ID,
    display_name: "Camera",
    icon: "tabler:camera",
    params: PARAMS,
    capture_kind: CaptureKind::Camera,
    default_transform: selfie_flip,
};

pub fn register() -> VoidRegistration {
    video_stream_void::registration(&CONFIG, |params, shared| {
        video_stream_void::build_void(&CONFIG, params, shared)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::params::ParamValue;

    #[test]
    fn config_matches_type_id() {
        assert_eq!(CONFIG.type_id, TYPE_ID);
        assert_eq!(register().type_id, "camera");
    }

    #[test]
    fn params_are_freeze_and_divisor() {
        let names: Vec<_> = PARAMS.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["freeze", "frame_divisor"]);
    }

    #[test]
    fn seeds_horizontal_flip() {
        // The selfie flip negates x-scale and translates by canvas width, so a
        // 200-wide canvas maps x=0 → 200 and x=200 → 0 (mirror about center).
        let t = selfie_flip(200, 100);
        let m = t.to_affine();
        assert_eq!(m, [-1.0, 0.0, 200.0, 0.0, 1.0, 0.0]);
        assert_eq!(
            crate::transform::affine_transform(&m, 0.0, 50.0),
            (200.0, 50.0)
        );
        assert_eq!(
            crate::transform::affine_transform(&m, 200.0, 50.0),
            (0.0, 50.0)
        );
    }

    #[test]
    fn default_divisor_round_trips_through_registration() {
        // The registration's default param values match the schema.
        let reg = register();
        let defaults: Vec<ParamValue> = reg.params.iter().map(|d| d.default_value()).collect();
        assert_eq!(defaults[0], ParamValue::Bool(false));
        assert_eq!(defaults[1], ParamValue::Int(4));
    }
}
