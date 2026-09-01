//! The `.darkly-brush` archive — a brush pack, on disk and over the wire.
//!
//! There is one brush format and it is the pack. Exporting a single brush
//! produces a pack containing one brush, so there is one magic-byte case, one
//! importer, one writer, and one thing to explain to a painter. The extension
//! names a container rather than a count, the same way `.darkly` does for
//! layers.
//!
//! Layout:
//! ```text
//! pack.json                  — manifest: pack identity + the entry list
//! brushes/<brushId>.json     — one brush record per member, in member order
//! ```
//!
//! Entry paths are keyed by brush id, which is opaque and filename-safe by
//! construction, so no name sanitizing or collision suffixing is needed here.

use serde::{Deserialize, Serialize};

use crate::brush::metadata::BrushMetadata;
use crate::brush::pack::{validate_pack, BrushPack, PackPalette};
use crate::format::unzip::unzip_entries;
use crate::format::zip_io::write_entries;

/// Discriminates a pack archive from any other zip that reaches the importer.
pub const FORMAT_TAG: &str = "darkly-brush";

/// Archive schema version.
///
/// A discriminator, not a migration hook — the same policy `CONFIG_VERSION`
/// states. Pre-release, a mismatch is rejected outright rather than upgraded.
pub const PACK_VERSION: u32 = 1;

/// Zip entry path for the manifest.
const MANIFEST_PATH: &str = "pack.json";

/// Directory prefix for brush records inside the archive.
const BRUSH_DIR: &str = "brushes";

/// The manifest at the root of a pack archive.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PackManifest {
    format: String,
    version: u32,
    name: String,
    #[serde(default)]
    description: String,
    icon: String,
    palette: PackPalette,
    #[serde(default)]
    author: String,
    /// Entry paths of the member brushes, **in the pack's member order**.
    ///
    /// Paths only: each brush's id and name live in its own record, and
    /// repeating them here would be the same fact stored twice. The order is
    /// the pack's own data and lives nowhere else in the archive.
    brushes: Vec<String>,
}

/// A pack and its brushes, as an archive carries them.
///
/// Deliberately not a [`BrushPack`]: an archive carries no id (the importer
/// always mints a fresh one) and no mutability (an imported pack is always the
/// painter's own, hence always `Full`). Writing either would invite a
/// hand-edited value the engine would have to distrust.
#[derive(Clone, Debug)]
pub struct PackFile {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub palette: PackPalette,
    pub author: String,
    /// Member brushes, in the pack's member order.
    pub brushes: Vec<BrushMetadata>,
}

impl PackFile {
    /// Build an archive payload from a pack and the brushes it names.
    ///
    /// `brushes` must already be in member order — the library resolves member
    /// ids to records, and a member it cannot resolve is simply absent.
    pub fn new(pack: &BrushPack, brushes: Vec<BrushMetadata>) -> Self {
        PackFile {
            name: pack.name.clone(),
            description: pack.description.clone(),
            icon: pack.icon.clone(),
            palette: pack.palette.clone(),
            author: String::new(),
            brushes,
        }
    }

    fn entry_path(id: &str) -> String {
        format!("{BRUSH_DIR}/{id}.json")
    }

    /// Serialize to `.darkly-brush` zip bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        validate_pack(&self.name, &self.icon, &self.palette)?;

        let manifest = PackManifest {
            format: FORMAT_TAG.to_string(),
            version: PACK_VERSION,
            name: self.name.clone(),
            description: self.description.clone(),
            icon: self.icon.clone(),
            palette: self.palette.clone(),
            author: self.author.clone(),
            brushes: self
                .brushes
                .iter()
                .map(|b| Self::entry_path(&b.id))
                .collect(),
        };

        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("failed to serialize pack manifest: {e}"))?;

        // Own every buffer first: `write_entries` borrows, so the encoded
        // records must outlive the entry list.
        let records: Vec<(String, Vec<u8>)> = self
            .brushes
            .iter()
            .map(|b| {
                serde_json::to_vec_pretty(b)
                    .map(|json| (Self::entry_path(&b.id), json))
                    .map_err(|e| format!("failed to serialize brush '{}': {e}", b.name))
            })
            .collect::<Result<_, _>>()?;

        let mut entries: Vec<(&str, &[u8])> = vec![(MANIFEST_PATH, &manifest_json)];
        entries.extend(records.iter().map(|(p, b)| (p.as_str(), b.as_slice())));

        write_entries(&entries, zip::CompressionMethod::Deflated)
            .map_err(|e| format!("failed to write pack archive: {e}"))
    }

    /// Deserialize from `.darkly-brush` zip bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let entries =
            unzip_entries(bytes).map_err(|e| format!("not a readable brush pack: {e}"))?;

        let manifest_bytes = entries
            .get(MANIFEST_PATH)
            .ok_or_else(|| format!("missing {MANIFEST_PATH} — not a brush pack"))?;
        let manifest: PackManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|e| format!("invalid {MANIFEST_PATH}: {e}"))?;

        if manifest.format != FORMAT_TAG {
            return Err(format!(
                "'{}' is not a brush pack (format tag '{}')",
                manifest.name, manifest.format
            ));
        }
        if manifest.version != PACK_VERSION {
            return Err(format!(
                "brush pack '{}' is version {}, but this build reads version {PACK_VERSION}",
                manifest.name, manifest.version
            ));
        }

        validate_pack(&manifest.name, &manifest.icon, &manifest.palette)?;

        let mut brushes = Vec::with_capacity(manifest.brushes.len());
        for path in &manifest.brushes {
            // A manifest naming an entry the archive does not hold is a
            // truncated file — reject it rather than import half a pack.
            let record = entries
                .get(path)
                .ok_or_else(|| format!("brush pack names '{path}', which the archive lacks"))?;
            let metadata: BrushMetadata = serde_json::from_slice(record)
                .map_err(|e| format!("invalid brush record '{path}': {e}"))?;
            if metadata.id.trim().is_empty() {
                return Err(format!("brush record '{path}' has no id"));
            }
            brushes.push(metadata);
        }

        Ok(PackFile {
            name: manifest.name,
            description: manifest.description,
            icon: manifest.icon,
            palette: manifest.palette,
            author: manifest.author,
            brushes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;
    use crate::brush::pack::BrushPack;
    use crate::format::zip_io::write_entries;

    fn pack() -> BrushPack {
        let mut p = BrushPack::new(
            "p1",
            "Watercolors",
            "mdi:water",
            PackPalette::new("#2f7fe0", "#2fd0c0", "#0c1a26"),
        );
        p.description = "Wet pigment that pools and blends.".into();
        p.members = vec!["a".into(), "b".into()];
        p
    }

    fn brushes() -> Vec<BrushMetadata> {
        vec![
            BrushMetadata::from_graph("a", "Rough Watercolor", brush::default_graph()),
            BrushMetadata::from_graph("b", "Smooth Watercolor", brush::default_graph()),
        ]
    }

    #[test]
    fn pack_round_trips_through_bytes() {
        let file = PackFile::new(&pack(), brushes());
        let bytes = file.to_bytes().unwrap();
        let back = PackFile::from_bytes(&bytes).unwrap();

        assert_eq!(back.name, "Watercolors");
        assert_eq!(back.description, "Wet pigment that pools and blends.");
        assert_eq!(back.icon, "mdi:water");
        // Every role survives the archive, not just the two that used to exist.
        assert_eq!(
            back.palette,
            PackPalette::new("#2f7fe0", "#2fd0c0", "#0c1a26")
        );

        // Member order is the pack's own data and must survive.
        let ids: Vec<&str> = back.brushes.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(back.brushes[0].name, "Rough Watercolor");

        // Every graph survives intact.
        for (before, after) in file.brushes.iter().zip(&back.brushes) {
            assert_eq!(
                serde_json::to_value(&before.graph).unwrap(),
                serde_json::to_value(&after.graph).unwrap(),
                "graph for '{}'",
                before.id
            );
        }
    }

    #[test]
    fn pack_of_one_round_trips() {
        // Exporting a single brush is a pack of one — the whole reason there
        // is only one format.
        let mut p = pack();
        p.members = vec!["a".into()];
        let one = vec![BrushMetadata::from_graph(
            "a",
            "Ink Pen",
            brush::default_graph(),
        )];
        let bytes = PackFile::new(&p, one).to_bytes().unwrap();

        let back = PackFile::from_bytes(&bytes).unwrap();
        assert_eq!(back.brushes.len(), 1);
        assert_eq!(back.brushes[0].name, "Ink Pen");
    }

    #[test]
    fn an_empty_pack_round_trips() {
        let mut p = pack();
        p.members.clear();
        let bytes = PackFile::new(&p, vec![]).to_bytes().unwrap();
        assert!(PackFile::from_bytes(&bytes).unwrap().brushes.is_empty());
    }

    #[test]
    fn corrupt_zip_returns_error() {
        let err = PackFile::from_bytes(b"not a zip at all").unwrap_err();
        assert!(err.contains("not a readable brush pack"), "got: {err}");
    }

    #[test]
    fn missing_pack_json_returns_error() {
        let bytes = write_entries(
            &[("something-else.txt", b"hello")],
            zip::CompressionMethod::Deflated,
        )
        .unwrap();
        let err = PackFile::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("missing pack.json"), "got: {err}");
    }

    /// A shape-valid palette, as a manifest carries one. Hand-written so the
    /// rejection cases below each fail for the reason they name.
    fn palette_json() -> serde_json::Value {
        serde_json::json!({
            "chroma": "#2f7fe0", "refraction": "#2fd0c0", "surface": "#0c1a26",
        })
    }

    /// Build an archive from a hand-written manifest, for the rejection cases.
    fn archive_with_manifest(manifest: serde_json::Value) -> Vec<u8> {
        let json = serde_json::to_vec(&manifest).unwrap();
        write_entries(&[(MANIFEST_PATH, &json)], zip::CompressionMethod::Deflated).unwrap()
    }

    #[test]
    fn version_mismatch_is_rejected() {
        let bytes = archive_with_manifest(serde_json::json!({
            "format": FORMAT_TAG, "version": 2, "name": "Future",
            "icon": "mdi:water", "palette": palette_json(),
            "brushes": [],
        }));
        let err = PackFile::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("version 2"), "got: {err}");
    }

    #[test]
    fn a_foreign_format_tag_is_rejected() {
        // A `.darkly` document is also a zip; the tag is what tells them apart.
        let bytes = archive_with_manifest(serde_json::json!({
            "format": "darkly-document", "version": 1, "name": "Doc",
            "icon": "mdi:water", "palette": palette_json(),
            "brushes": [],
        }));
        let err = PackFile::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("is not a brush pack"), "got: {err}");
    }

    #[test]
    fn a_manifest_naming_a_missing_entry_is_rejected() {
        let bytes = archive_with_manifest(serde_json::json!({
            "format": FORMAT_TAG, "version": PACK_VERSION, "name": "Truncated",
            "icon": "mdi:water", "palette": palette_json(),
            "brushes": ["brushes/gone.json"],
        }));
        let err = PackFile::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("which the archive lacks"), "got: {err}");
    }

    #[test]
    fn a_malformed_manifest_color_is_rejected() {
        let bytes = archive_with_manifest(serde_json::json!({
            "format": FORMAT_TAG, "version": PACK_VERSION, "name": "Bad",
            "icon": "mdi:water", "palette": { "chroma": "not-a-color",
                "refraction": "#2fd0c0", "surface": "#0c1a26" },
            "brushes": [],
        }));
        assert!(PackFile::from_bytes(&bytes).is_err());
    }

    #[test]
    fn an_unqualified_manifest_icon_is_rejected() {
        let bytes = archive_with_manifest(serde_json::json!({
            "format": FORMAT_TAG, "version": PACK_VERSION, "name": "Bad",
            "icon": "star", "palette": palette_json(),
            "brushes": [],
        }));
        assert!(PackFile::from_bytes(&bytes).is_err());
    }

    #[test]
    fn a_manifest_missing_a_palette_role_is_rejected() {
        // Pins "no defaulting, no migration": a role absent from the manifest is
        // a rejection, not a silently blackened pack.
        let bytes = archive_with_manifest(serde_json::json!({
            "format": FORMAT_TAG, "version": PACK_VERSION, "name": "Partial",
            "icon": "mdi:water",
            "palette": { "chroma": "#2f7fe0", "refraction": "#2fd0c0" },
            "brushes": [],
        }));
        let err = PackFile::from_bytes(&bytes).unwrap_err();
        assert!(
            err.contains("surface"),
            "error should name the missing role: {err}"
        );
    }

    #[test]
    fn a_translucent_surface_round_trips() {
        // Alpha is a pack's way of letting the app's background through, so it
        // has to survive the archive rather than being normalized away.
        let mut p = pack();
        p.palette.surface = "#2a2148cc".into();
        let bytes = PackFile::new(&p, brushes()).to_bytes().unwrap();
        let back = PackFile::from_bytes(&bytes).unwrap();
        assert_eq!(back.palette.surface, "#2a2148cc");
    }

    #[test]
    fn an_icon_the_renderer_lacks_still_imports() {
        // Shape is the format's business; renderability is the renderer's, and
        // it falls back rather than showing a hole. A third-party pack must
        // degrade, not fail.
        let bytes = archive_with_manifest(serde_json::json!({
            "format": FORMAT_TAG, "version": PACK_VERSION, "name": "Exotic",
            "icon": "some-collection:nonexistent",
            "palette": palette_json(),
            "brushes": [],
        }));
        assert_eq!(PackFile::from_bytes(&bytes).unwrap().name, "Exotic");
    }
}
