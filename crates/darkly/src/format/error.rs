//! Error types surfaced by the `.darkly` load path.
//!
//! Every refusal carries enough information for the UI's `LoadErrorToast`
//! (Phase 5) to format a precise diagnostic — "this file needs
//! `veil/lens_flare`, please update Darkly" rather than "load failed."
//!
//! The variants here are the full closed set for load; the save path is
//! infallible from the engine's perspective (any GPU failure is logged and
//! surfaces empty bytes).

use std::fmt;

/// All ways a `.darkly` load can fail.
#[derive(Debug)]
pub enum LoadError {
    /// The file's `container_version` is newer than the binary understands.
    /// Bumped only for fundamental container-structure breaks (Phase 4
    /// pre-check).
    ContainerTooNew { found: u32, supported: u32 },

    /// The file's `requires` inventory names features the binary's
    /// registries don't know about. Each entry is `"<registry>/<type_id>"`
    /// — e.g. `"veil/lens_flare"`, `"blend_mode/divide"`,
    /// `"layer_kind/text"`, `"modifier/clip"`.
    UnsupportedFeatures { missing: Vec<String> },

    /// The `requires` inventory was absent, malformed, or disagreed with
    /// the body (caught by the per-variant safety net during deserialize).
    /// We control the writer; an absent `requires` is malformed, not
    /// "older format" — there is no older format.
    CorruptManifest { reason: String },

    /// Encountered a `type_id` in the body that isn't in the binary's
    /// registry for that `kind`. This is the per-variant safety net for
    /// files whose `requires` block was hand-edited or otherwise lies.
    UnknownTypeId { kind: &'static str, id: String },

    /// I/O error reading the file bytes off disk.
    Io(std::io::Error),

    /// The container ZIP was unreadable.
    Zip(String),

    /// The `manifest.json` failed to parse as JSON or didn't match the
    /// expected schema.
    Json(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::ContainerTooNew { found, supported } => write!(
                f,
                "container version {found} is newer than supported version {supported}; \
                 please update Darkly to open this file"
            ),
            LoadError::UnsupportedFeatures { missing } => write!(
                f,
                "this file needs features your build doesn't support: {}; \
                 please update Darkly to open this file",
                missing.join(", ")
            ),
            LoadError::CorruptManifest { reason } => write!(f, "corrupt manifest: {reason}"),
            LoadError::UnknownTypeId { kind, id } => {
                write!(f, "unknown {kind} type id: {id}")
            }
            LoadError::Io(e) => write!(f, "io error: {e}"),
            LoadError::Zip(e) => write!(f, "zip error: {e}"),
            LoadError::Json(e) => write!(f, "manifest json error: {e}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        LoadError::Io(e)
    }
}

impl From<serde_json::Error> for LoadError {
    fn from(e: serde_json::Error) -> Self {
        LoadError::Json(e.to_string())
    }
}

impl LoadError {
    /// Stable wire shape for the JS-side UI. The UI's `LoadErrorToast`
    /// switches on `kind` to format the precise diagnostic — "please
    /// update Darkly" for `containerTooNew` / `unsupportedFeatures`,
    /// "this file is malformed" for `corruptManifest`, raw message for
    /// `io` / `zip` / `json`.
    ///
    /// Returned via the WASM bridge as a JSON string inside the
    /// `JsError` payload (rather than serde-deriving on the enum
    /// itself — `std::io::Error` doesn't serialize and we'd rather
    /// keep the structured payload narrowly scoped to the UI contract).
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            LoadError::ContainerTooNew { found, supported } => json!({
                "kind": "containerTooNew",
                "found": found,
                "supported": supported,
                "message": self.to_string(),
            }),
            LoadError::UnsupportedFeatures { missing } => json!({
                "kind": "unsupportedFeatures",
                "missing": missing,
                "message": self.to_string(),
            }),
            LoadError::CorruptManifest { reason } => json!({
                "kind": "corruptManifest",
                "reason": reason,
                "message": self.to_string(),
            }),
            LoadError::UnknownTypeId { kind, id } => json!({
                "kind": "unknownTypeId",
                "registry": kind,
                "id": id,
                "message": self.to_string(),
            }),
            LoadError::Io(e) => json!({
                "kind": "io",
                "message": e.to_string(),
            }),
            LoadError::Zip(msg) => json!({
                "kind": "zip",
                "message": msg,
            }),
            LoadError::Json(msg) => json!({
                "kind": "json",
                "message": msg,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_diagnostic_for_unsupported_features() {
        let err = LoadError::UnsupportedFeatures {
            missing: vec!["veil/lens_flare".into(), "blend_mode/divide".into()],
        };
        let msg = format!("{err}");
        assert!(msg.contains("veil/lens_flare"));
        assert!(msg.contains("blend_mode/divide"));
        assert!(msg.contains("update Darkly"));
    }

    #[test]
    fn display_includes_versions_for_container_too_new() {
        let err = LoadError::ContainerTooNew {
            found: 999,
            supported: 1,
        };
        let msg = format!("{err}");
        assert!(msg.contains("999"));
        assert!(msg.contains("supported version 1"));
    }
}
