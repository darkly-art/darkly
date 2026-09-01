//! Blender void: a live Blender view as a layer.
//!
//! Sibling of the camera and screenshare voids: a thin config over the shared
//! [`crate::gpu::textured_void`] machinery. Frames arrive not from a browser
//! `MediaStream` but from an HTTP stream ([`CaptureKind::Stream`]) served by the
//! companion Blender add-on (`blender-addon/`), which captures the 3D viewport's
//! own view (or a camera POV), encodes alpha-carrying PNG, and streams
//! length-prefixed frames over localhost. The browser's `createImageBitmap`
//! decodes each frame (with transparency) straight into the same
//! `ImageBitmap → copy_external_image_to_texture` path camera/screenshare use, so
//! the GPU/void/compositor side needs nothing new.
//!
//! The `url` param is the one addition over the other stream voids: it's a
//! document-persisted value the **frontend** reads to know where to connect. The
//! Rust void looks its params up by name (`freeze`, `frame_divisor`) and never
//! reads `url`; it exists purely as frontend-facing document state.

use crate::gpu::textured_void::ContentFit;
use crate::gpu::textured_void::{self, TexturedVoidConfig};
use crate::gpu::void::{CaptureKind, ParamDef, VoidRegistration, VoidSource};

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
    ParamDef::boolean("freeze", false)
        .with_label("Freeze")
        .with_description("Holds the last received frame instead of following the live stream."),
    // rAF frames to skip between decoded-frame → GPU uploads (see camera void).
    ParamDef::int("frame_divisor", 1, 60, 4)
        .with_label("Frame Skip")
        .with_description("Take one frame in this many, to lighten the load."),
    // Where the frontend `fetch`es the frame stream. Not read by the Rust void;
    // `TexturedVoid` resolves params by name and ignores this one; it's
    // document-persisted purely so the frontend knows where to connect and so
    // the endpoint round-trips through save/load.
    ParamDef::string("url", DEFAULT_URL)
        .with_label("Stream URL")
        .with_description("Address the Blender frame stream is served from."),
];

static CONFIG: TexturedVoidConfig = TexturedVoidConfig {
    type_id: TYPE_ID,
    display_name: "Blender",
    description: "Live viewport frames streamed from a running Blender session.",
    icon: "file-icons:blender",
    params: PARAMS,
    source: VoidSource::Capture {
        capture: CaptureKind::Stream,
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
        assert_eq!(reg.type_id, "blender");
        assert_eq!(reg.display_name, "Blender");
        assert_eq!(reg.source.capture_kind(), Some(CaptureKind::Stream));
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
        let names: Vec<_> = reg.params.iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["freeze", "frame_divisor", "url"]);

        let defaults: Vec<ParamValue> = reg.params.iter().map(|d| d.default_value()).collect();
        assert_eq!(defaults[0], ParamValue::Bool(false));
        assert_eq!(defaults[1], ParamValue::Int(4));
        assert_eq!(defaults[2], ParamValue::String(DEFAULT_URL.to_string()));
    }
}
