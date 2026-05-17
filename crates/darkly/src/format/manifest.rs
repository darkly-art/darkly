//! Wire-format schema for `manifest.json` inside a `.darkly` ZIP.
//!
//! These are pure data structs — they round-trip through serde and never
//! reference GPU state or registry pointers directly. The save path
//! walks the in-memory [`crate::document::Document`] and builds a
//! [`Manifest`]; the load path parses a [`Manifest`] back into a staging
//! [`crate::document::Document`] with a fresh slotmap.
//!
//! Identifiers on disk are plain numeric ids (`u64`), not slotmap keys —
//! the on-load id remap is what reconciles them with a fresh
//! `Document::entities`.
//!
//! Each entity (layer kind or modifier kind) crosses the wire as a
//! [`ManifestEntry`] envelope: `{ id, type: <type_id>, body: <opaque value> }`.
//! The `body` is read and written only by the kind's own registration
//! functions; central save/load code never branches on which variant it
//! got. Per-kind body structs (`RasterBody`, `MaskBody`, …) live inside
//! each module under [`crate::document::layer_kinds`] /
//! [`crate::document::modifiers`].

use serde::{Deserialize, Serialize};

use super::registry_io::InstancePayload;
use crate::coord::CanvasRect;

/// Current container schema version. Bumped *only* for fundamental
/// container-structure breaks — see [the plan's "two version concepts"
/// section](../../../../darkly-file-format-plan.md#two-version-concepts-kept-strictly-separate).
/// Expect zero bumps in the near term.
pub const CONTAINER_VERSION: u32 = 1;

/// Magic value for the `format` field. Identifies the file as a Darkly
/// document; readers refuse anything else.
pub const FORMAT_TAG: &str = "darkly";

/// Top-level manifest written as `manifest.json` inside the `.darkly` ZIP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Fixed string `"darkly"` — guards against opening unrelated zips.
    pub format: String,
    /// Container schema version. See [`CONTAINER_VERSION`].
    pub container_version: u32,
    /// Tooling that produced this file (informational).
    pub writer: ManifestWriter,
    /// User-visible document name. Defaults to `"Untitled"` for fresh
    /// documents; updated by `set_document_name` or the Save As picker.
    pub name: String,
    /// Canvas dimensions in pixels.
    pub canvas: ManifestCanvas,
    /// Inventory of modular features the file uses — see
    /// [`ManifestRequires`].
    pub requires: ManifestRequires,
    /// Path inside the zip to a baked composite PNG (always written).
    /// For external interop only; the loader never reads it.
    pub composite: String,
    /// Manifest id of the implicit root group inside [`Self::nodes`].
    pub root: u64,
    /// Every tree node (layers + groups) in the document, in id order.
    /// Each entry's `body` is opaque to the central code — only the
    /// layer kind's `serialize` / `deserialize` touch its contents.
    pub nodes: Vec<ManifestEntry>,
    /// Every modifier (mask, selection, future filter/transform/…),
    /// in id order. Same envelope contract as [`Self::nodes`].
    pub modifiers: Vec<ManifestEntry>,
    /// Manifest id of the global selection modifier, if one exists.
    /// The modifier itself lives in [`Self::modifiers`]; this is just a
    /// pointer so the loader can find it without scanning the list.
    #[serde(default)]
    pub selection_id: Option<u64>,
    /// Veil chain in apply order.
    pub veils: Vec<ManifestVeil>,
}

/// Wire envelope for one entity (layer kind or modifier kind). The
/// `body` is opaque to the central save/load code — only the kind's
/// own registered `serialize` / `deserialize` functions read or write
/// it.
///
/// Named `ManifestEntry` (not `ManifestEntity`) to avoid colliding
/// with the document-side `Entity::{Node, Modifier}` enum — they're
/// different abstractions: `Entity` is the in-doc storage
/// discriminator; `ManifestEntry` is the wire envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: u64,
    #[serde(rename = "type")]
    pub type_id: String,
    pub body: serde_json::Value,
}

/// Tooling identifier — `name` is fixed `"darkly"`; `version` mirrors
/// the crate version so files leave a breadcrumb of which build wrote
/// them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestWriter {
    pub name: String,
    pub version: String,
}

impl ManifestWriter {
    /// Writer block for the running build — `name = "darkly"`, `version`
    /// from the crate's `Cargo.toml`.
    pub fn current() -> Self {
        ManifestWriter {
            name: "darkly".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestCanvas {
    pub width: u32,
    pub height: u32,
}

/// Inventory of every modular `type_id` the file uses, keyed by registry.
///
/// Populated automatically by the save path — walked from the document,
/// never hand-maintained. The load path diffs this against the binary's
/// registries and refuses up-front when anything's missing, so big files
/// get rejected in milliseconds without parsing the body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRequires {
    #[serde(default)]
    pub veil: Vec<String>,
    #[serde(default)]
    pub blend_mode: Vec<String>,
    #[serde(default)]
    pub layer_kind: Vec<String>,
    #[serde(default)]
    pub modifier: Vec<String>,
}

/// On-disk shape for a veil chain entry. The body is the canonical
/// `{ type_id, params }` envelope used by every modular system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestVeil {
    #[serde(flatten)]
    pub instance: InstancePayload,
    /// Veils carry per-instance visibility — the chain can disable an
    /// effect without removing it.
    pub visible: bool,
}

/// Reference to a pixel blob inside the zip.
///
/// `format` is the `wgpu::TextureFormat` slug (`"rgba8unorm"`,
/// `"r8unorm"`, future `"r16unorm"` etc.) — adding a new on-GPU format
/// only adds a string variant here, never a new file extension. `pixels`
/// is the zip-relative path; `bounds` is canvas-space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPixelRef {
    pub format: String,
    pub pixels: String,
    pub bounds: CanvasRect,
}

/// Output of a completed save — the Rust data path's hand-off shape.
///
/// JS consumes this directly: PNG-encodes `composite_rgba` via
/// `OffscreenCanvas` (and a downsampled thumbnail) and assembles the zip
/// with `fflate`. Rust never writes the zip in production — keeping
/// encoders off the WASM main thread and reusing the browser's native
/// PNG path. Tests assemble the zip via [`super::zip_io::assemble_zip`].
///
/// Pixel blobs are *raw bytes*, format declared per-blob in each entity's
/// body inside [`Manifest::nodes`] / [`Manifest::modifiers`].
pub struct SaveBundle {
    /// `manifest.json` content — pretty-printed JSON.
    pub manifest_json: Vec<u8>,
    pub composite_width: u32,
    pub composite_height: u32,
    /// Raw RGBA8 bytes of the composited canvas (one byte-per-channel,
    /// no row padding). JS PNG-encodes this via `OffscreenCanvas`.
    pub composite_rgba: Vec<u8>,
    /// Per-layer / per-mask / selection raw pixel blobs, keyed by
    /// zip-relative path matching the corresponding `ManifestPixelRef::pixels`
    /// in the manifest.
    pub blobs: Vec<SaveBlob>,
}

/// One named raw-pixel entry inside a [`SaveBundle`].
pub struct SaveBlob {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Map a `wgpu::TextureFormat` to its wire-format slug.
///
/// Closed set today: `Rgba8Unorm` ↔ `"rgba8unorm"`, `R8Unorm` ↔
/// `"r8unorm"`. Panics on anything else — the compositor allocates
/// only representable formats (raster textures are always `Rgba8Unorm`,
/// mask/selection textures are always `R8Unorm`), so an unknown format
/// reaching this function is a programming error, not a save-time
/// concern. Adding a new on-GPU format requires extending this map.
pub fn texture_format_to_str(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => "rgba8unorm",
        wgpu::TextureFormat::R8Unorm => "r8unorm",
        other => panic!("texture_format_to_str: unrepresentable format {other:?}"),
    }
}

/// Inverse of [`texture_format_to_str`]. Returns `None` for any string
/// the binary doesn't recognize — the load path turns that into a
/// structured [`super::error::LoadError`].
pub fn texture_format_from_str(slug: &str) -> Option<wgpu::TextureFormat> {
    match slug {
        "rgba8unorm" => Some(wgpu::TextureFormat::Rgba8Unorm),
        "r8unorm" => Some(wgpu::TextureFormat::R8Unorm),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_format_slug_round_trip() {
        for fmt in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::R8Unorm,
        ] {
            let slug = texture_format_to_str(fmt);
            assert_eq!(texture_format_from_str(slug), Some(fmt));
        }
        assert!(texture_format_from_str("future_format").is_none());
    }

    #[test]
    #[should_panic(expected = "unrepresentable format")]
    fn texture_format_to_str_panics_on_unknown() {
        let _ = texture_format_to_str(wgpu::TextureFormat::Rgba16Float);
    }

    #[test]
    fn full_manifest_round_trip() {
        let m = Manifest {
            format: FORMAT_TAG.into(),
            container_version: CONTAINER_VERSION,
            writer: ManifestWriter::current(),
            name: "Untitled".into(),
            canvas: ManifestCanvas {
                width: 2048,
                height: 2048,
            },
            requires: ManifestRequires {
                veil: vec!["noise".into()],
                blend_mode: vec!["normal".into(), "multiply".into()],
                layer_kind: vec!["raster".into(), "group".into()],
                modifier: vec!["mask".into()],
            },
            composite: "composite.png".into(),
            root: 0,
            nodes: vec![ManifestEntry {
                id: 0,
                type_id: "group".into(),
                body: serde_json::json!({
                    "name": "Root",
                    "visible": true,
                    "locked": false,
                    "opacity": 1.0,
                    "blend_mode": "normal",
                    "passthrough": true,
                    "collapsed": false,
                    "children": [],
                    "modifiers": [],
                }),
            }],
            modifiers: vec![],
            selection_id: None,
            veils: vec![ManifestVeil {
                instance: InstancePayload::new(
                    "noise",
                    vec![
                        crate::gpu::params::ParamValue::Float(0.5),
                        crate::gpu::params::ParamValue::Float(0.05),
                    ],
                ),
                visible: true,
            }],
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }
}
