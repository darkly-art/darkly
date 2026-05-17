//! Wire-format schema for `manifest.json` inside a `.darkly` ZIP.
//!
//! These are pure data structs — they round-trip through serde and never
//! reference GPU state or registry pointers directly. The save path
//! (Phase 3) walks the in-memory [`crate::document::Document`] and builds
//! a [`Manifest`]; the load path (Phase 4) parses a [`Manifest`] back into
//! a staging [`crate::document::Document`] with a fresh slotmap.
//!
//! Identifiers on disk are plain numeric ids (`u64`), not slotmap keys —
//! the on-load id remap (Phase 4) is what reconciles them with a fresh
//! `Document::entities`.

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
    /// Layer tree — list of nodes plus the root id.
    pub tree: ManifestTree,
    /// Modifiers attached to nodes in the tree, plus the global selection
    /// modifier if present. Modifiers live in their own list because they
    /// share an id space with tree nodes but aren't traversed as part of
    /// the tree.
    pub modifiers: Vec<ManifestModifier>,
    /// Global selection modifier metadata, if the document had a
    /// non-empty selection at save time.
    pub selection: Option<ManifestSelection>,
    /// Veil chain in apply order.
    pub veils: Vec<ManifestVeil>,
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
/// Populated automatically by the save path (Phase 3) — walked from the
/// document, never hand-maintained. The load path (Phase 4) diffs this
/// against the binary's registries and refuses up-front when anything's
/// missing, so big files get rejected in milliseconds without parsing
/// the body.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestTree {
    pub root: u64,
    pub nodes: Vec<ManifestNode>,
}

/// On-disk shape for any tree node. `kind` is the layer-kind `type_id`
/// (`"raster"`, `"group"`), dispatched through
/// [`crate::document::layer_kind`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestNode {
    Raster(ManifestRasterNode),
    Group(ManifestGroupNode),
}

impl ManifestNode {
    pub fn id(&self) -> u64 {
        match self {
            ManifestNode::Raster(r) => r.id,
            ManifestNode::Group(g) => g.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestRasterNode {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    /// Blend-mode `type_id` from [`crate::gpu::blend_mode`].
    pub blend_mode: String,
    /// Pixel storage descriptor — bounds + format + zip-relative path.
    pub pixels: ManifestPixelRef,
    /// Modifier ids attached to this node, in bottom-up order. Each id
    /// resolves to an entry in [`Manifest::modifiers`].
    #[serde(default)]
    pub modifiers: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestGroupNode {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub blend_mode: String,
    pub passthrough: bool,
    pub collapsed: bool,
    /// Child node ids in display order (bottom-to-top).
    pub children: Vec<u64>,
    #[serde(default)]
    pub modifiers: Vec<u64>,
}

/// On-disk shape for any modifier. `kind` is the modifier-kind `type_id`
/// (`"mask"`, `"selection"`, …), dispatched through
/// [`crate::document::modifier`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestModifier {
    Mask(ManifestMaskModifier),
    Selection(ManifestSelectionModifier),
}

impl ManifestModifier {
    pub fn id(&self) -> u64 {
        match self {
            ManifestModifier::Mask(m) => m.id,
            ManifestModifier::Selection(s) => s.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestMaskModifier {
    pub id: u64,
    /// Host node id — the layer or group this mask multiplies into.
    pub host: u64,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub pixels: ManifestPixelRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestSelectionModifier {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub pixels: ManifestPixelRef,
}

/// Schema for [`Manifest::selection`] — a compact descriptor for the
/// global selection mask if non-empty at save time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestSelection {
    pub pixels: ManifestPixelRef,
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

/// Map a `wgpu::TextureFormat` to its wire-format slug.
///
/// Closed set today: `Rgba8Unorm` ↔ `"rgba8unorm"`, `R8Unorm` ↔
/// `"r8unorm"`. Anything else returns `None` — we don't write unknown
/// formats so an unknown one on save is a programming error.
pub fn texture_format_to_str(format: wgpu::TextureFormat) -> Option<&'static str> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => Some("rgba8unorm"),
        wgpu::TextureFormat::R8Unorm => Some("r8unorm"),
        _ => None,
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
    fn manifest_node_uses_kind_tag() {
        let node = ManifestNode::Raster(ManifestRasterNode {
            id: 1,
            name: "Sketch".into(),
            visible: true,
            locked: false,
            opacity: 0.8,
            blend_mode: "multiply".into(),
            pixels: ManifestPixelRef {
                format: "rgba8unorm".into(),
                pixels: "layers/0.pixels".into(),
                bounds: CanvasRect::from_xywh(0, 0, 2048, 2048),
            },
            modifiers: vec![3],
        });
        let json = serde_json::to_value(&node).unwrap();
        assert_eq!(json["kind"], "raster");
        let back: ManifestNode = serde_json::from_value(json).unwrap();
        assert_eq!(back, node);
    }

    #[test]
    fn manifest_modifier_uses_kind_tag() {
        let m = ManifestModifier::Mask(ManifestMaskModifier {
            id: 3,
            host: 1,
            name: "Mask 1".into(),
            visible: true,
            locked: false,
            pixels: ManifestPixelRef {
                format: "r8unorm".into(),
                pixels: "layers/0.mask.pixels".into(),
                bounds: CanvasRect::from_xywh(0, 0, 2048, 2048),
            },
        });
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["kind"], "mask");
        let back: ManifestModifier = serde_json::from_value(json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn texture_format_slug_round_trip() {
        for fmt in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::R8Unorm,
        ] {
            let slug = texture_format_to_str(fmt).unwrap();
            assert_eq!(texture_format_from_str(slug), Some(fmt));
        }
        assert!(texture_format_from_str("future_format").is_none());
        assert!(texture_format_to_str(wgpu::TextureFormat::Rgba16Float).is_none());
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
            tree: ManifestTree {
                root: 0,
                nodes: vec![ManifestNode::Group(ManifestGroupNode {
                    id: 0,
                    name: "Root".into(),
                    visible: true,
                    locked: false,
                    opacity: 1.0,
                    blend_mode: "normal".into(),
                    passthrough: true,
                    collapsed: false,
                    children: vec![],
                    modifiers: vec![],
                })],
            },
            modifiers: vec![],
            selection: None,
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
