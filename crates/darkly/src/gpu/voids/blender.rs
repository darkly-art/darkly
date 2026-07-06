//! Blender void — a live Blender camera render as a layer.
//!
//! Sibling of the camera and screenshare voids: a thin config over the shared
//! [`crate::gpu::video_stream_void`] machinery. Frames arrive not from a browser
//! `MediaStream` but from an HTTP stream ([`CaptureKind::Stream`]) served by the
//! companion Blender add-on (`blender-addon/`), which renders the active camera
//! to an offscreen buffer, encodes WebP-with-alpha, and streams length-prefixed
//! frames over localhost. The browser's `createImageBitmap` decodes each WebP
//! frame (with transparency) straight into the same
//! `ImageBitmap → copy_external_image_to_texture` path camera/screenshare use, so
//! the GPU/void/compositor side needs nothing new.
//!
//! The `url` param is the one addition over the other stream voids: it's a
//! document-persisted value the **frontend** reads to know where to connect. The
//! Rust void looks its params up by name (`freeze`, `frame_divisor`) and never
//! reads `url` — it exists purely as frontend-facing document state.

use crate::gpu::video_stream_void::{self, VideoStreamConfig};
use crate::gpu::void::{CaptureKind, ParamDef, VoidRegistration};

pub const TYPE_ID: &str = "blender";

/// Default localhost endpoint the Blender add-on serves its frame stream on.
/// Editable per-layer via the `url` string param in the properties panel.
pub const DEFAULT_URL: &str = "http://localhost:8765/stream";

// `freeze` + `frame_divisor` mirror the other stream voids (looked up by name by
// the shared machinery), plus a `url` the frontend reads to open the HTTP stream.
const PARAMS: &[ParamDef] = &[
    // Freeze on the last received frame; suppresses uploads (GPU holds the last
    // frame) while the frontend keeps the HTTP stream open, so unfreezing
    // resumes instantly (see camera void).
    ParamDef::Bool {
        name: "freeze",
        default: false,
    },
    // rAF frames to skip between decoded-frame → GPU uploads (see camera void).
    ParamDef::Int {
        name: "frame_divisor",
        min: 1,
        max: 60,
        default: 4,
    },
    // Where the frontend `fetch`es the frame stream. Not read by the Rust void —
    // `VideoStreamVoid` resolves params by name and ignores this one; it's
    // document-persisted purely so the frontend knows where to connect and so
    // the endpoint round-trips through save/load.
    ParamDef::String {
        name: "url",
        default: DEFAULT_URL,
    },
];

static CONFIG: VideoStreamConfig = VideoStreamConfig {
    type_id: TYPE_ID,
    display_name: "Blender",
    icon: "file-icons:blender",
    params: PARAMS,
    capture_kind: CaptureKind::Stream,
    default_transform: |_, _| crate::transform::Transform::identity(),
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
        let reg = register();
        assert_eq!(reg.type_id, "blender");
        assert_eq!(reg.display_name, "Blender");
        assert_eq!(reg.capture_kind, Some(CaptureKind::Stream));
    }

    #[test]
    fn seeds_identity_transform() {
        let t = (CONFIG.default_transform)(200, 100);
        assert_eq!(t, crate::transform::Transform::identity());
    }

    #[test]
    fn exposes_url_string_param_with_default() {
        // The `url` param is the frontend's connection endpoint. It must be a
        // String param carrying the localhost default so a freshly-created void
        // connects without the user typing anything.
        let reg = register();
        let names: Vec<_> = reg.params.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["freeze", "frame_divisor", "url"]);

        let defaults: Vec<ParamValue> = reg.params.iter().map(|d| d.default_value()).collect();
        assert_eq!(defaults[0], ParamValue::Bool(false));
        assert_eq!(defaults[1], ParamValue::Int(4));
        assert_eq!(defaults[2], ParamValue::String(DEFAULT_URL.to_string()));
    }
}
